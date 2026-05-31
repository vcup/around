//! Audio pipeline: Source → Codec → Output Sink.

use crate::ipc_types::PlaybackState;
use crate::output::AudioOutput;
use around_core::{AroundError, AudioStream, CodecRegistry, Source, SourceCapabilities};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct Engine {
  config: crate::config::EngineConfig,
  registry: CodecRegistry,
  running: Arc<AtomicBool>,
  device_lost: Arc<AtomicBool>,
  paused: Arc<AtomicBool>,
  seek_target_ms: Arc<Mutex<Option<u64>>>,
  shutdown: Arc<AtomicBool>,
}

pub struct PlaybackHandle {
  pub output_format: around_core::SampleSpec,
  running: Arc<AtomicBool>,
}

impl PlaybackHandle {
  pub fn stop(&self) {
    self.running.store(false, Ordering::SeqCst);
  }
  pub fn is_running(&self) -> bool {
    self.running.load(Ordering::SeqCst)
  }
}

impl Engine {
  pub fn new(config: crate::config::EngineConfig) -> Self {
    let mut registry = CodecRegistry::new();
    registry.push(around_codec_wav::WAV_INFO);
    Self {
      config,
      registry,
      device_lost: Arc::new(AtomicBool::new(false)),
      running: Arc::new(AtomicBool::new(false)),
      paused: Arc::new(AtomicBool::new(false)),
      seek_target_ms: Arc::new(Mutex::new(None)),
      shutdown: Arc::new(AtomicBool::new(false)),
    }
  }

  pub fn registry_mut(&mut self) -> &mut CodecRegistry {
    &mut self.registry
  }
  pub fn registry(&self) -> &CodecRegistry {
    &self.registry
  }

  // -----------------------------------------------------------------------
  // FR-002: Format detection + codec selection
  //
  // Algorithm (seekable sources):
  //
  //   1. Extension detection (zero I/O).
  //   2. Try extension-matched codecs immediately — no magic bytes read
  //      if one succeeds. This preserves the "extension first, zero I/O"
  //      design intent.
  //   3. If all extension codecs fail (or no extension): read magic bytes
  //      from a probe, detect format. If extension and magic disagree,
  //      log a warning (magic wins per FR-002).
  //   4. Try magic-matched codecs not yet attempted.
  //   5. Try all remaining codecs in registration order.
  //
  // Non-seekable sources: pre-read probe into PrefixReader, detect format,
  // try the best-matching codec. No multi-codec retry because open_fn
  // takes Box<dyn ReadSeek> by value (ownership lost on failure).
  // Full retry for non-seekable requires an API change deferred to
  // a future iteration.
  // -----------------------------------------------------------------------

  fn open_codec(
    registry: &CodecRegistry,
    source: &dyn Source,
    seekable: bool,
  ) -> Result<(AudioStream, &'static str), AroundError> {
    // ---- Phase 1: Extension detection (zero I/O) ----
    let ext = Path::new(&source.identifier())
      .extension()
      .and_then(|e| e.to_str())
      .map(|e| e.to_lowercase());

    let ext_codecs = registry.by_extension(ext.as_deref().unwrap_or(""));

    // ---- Phase 2: Try extension-matched codecs first ----
    // No I/O for magic bytes yet — we only open the source and pass it
    // to the codec. If a codec succeeds, we never read magic bytes.
    if !ext_codecs.is_empty() && seekable {
      for codec in &ext_codecs {
        let reader = source.open_seekable()?;
        match (codec.open_fn)(reader) {
          Ok(stream) => return Ok((stream, codec.name)),
          Err(_) => continue,
        }
      }
      // All extension-matched codecs failed. Fall through to magic.
    } else if !ext_codecs.is_empty() {
      // Non-seekable: try first extension-matched codec with PrefixReader.
      // (Only one attempt — ownership constraint.)
      let raw = source.open()?;
      let mut prefix = around_core::PrefixReader::new(raw, 65536);
      // Read probe for potential magic fallback.
      let mut probe = vec![0u8; 4096];
      let _ = prefix.read(&mut probe);
      prefix.rewind();
      // Try first ext codec.
      if let Some(codec) = ext_codecs.first() {
        match (codec.open_fn)(Box::new(prefix)) {
          Ok(stream) => return Ok((stream, codec.name)),
          Err(_) => {
            // PrefixReader consumed. Fall through to magic detection
            // using the probe we already read.
          }
        }
      }
      // Re-open for magic fallback on non-seekable.
      let raw = source.open()?;
      let mut prefix = around_core::PrefixReader::new(raw, 65536);
      let _ = prefix.read(&mut vec![0u8; 4096]);
      prefix.rewind();
      let magic_codecs = registry.by_magic(&probe);
      if let Some(codec) = magic_codecs.first() {
        return match (codec.open_fn)(Box::new(prefix)) {
          Ok(stream) => Ok((stream, codec.name)),
          Err(e) => Err(e),
        };
      }
      return Err(AroundError::UnsupportedFormat {
        format: ext,
        reason: "no codec could open the source".into(),
      });
    }

    // ---- Phase 3: Magic detection ----
    // We reach here if:
    //   (a) No extension match, OR
    //   (b) All extension-matched codecs failed (seekable path)
    //
    // Now we need I/O: read magic bytes, detect format.
    let (_probe, magic_codecs) = {
      let mut reader = source.open_seekable()?;
      let mut probe = vec![0u8; 4096];
      let _ = reader.read(&mut probe);
      reader
        .seek(std::io::SeekFrom::Start(0))
        .map_err(|e| AroundError::DecodeError {
          message: format!("failed to seek after probe: {}", e),
        })?;
      let magic = registry.by_magic(&probe);
      (probe, magic)
    };

    if ext_codecs.is_empty() && magic_codecs.is_empty() {
      return Err(AroundError::UnsupportedFormat {
        format: ext,
        reason: "no matching codec".into(),
      });
    }

    // Conflict: extension and magic disagree → warn, magic wins.
    if !ext_codecs.is_empty()
      && !magic_codecs.is_empty()
      && !same_codec_set(&ext_codecs, &magic_codecs)
    {
      tracing::warn!(
        "extension .{} disagrees with magic bytes, using magic-detected codec",
        ext.as_deref().unwrap_or("<none>")
      );
    }

    // ---- Phase 4: Try magic-matched codecs ----
    // Skip codecs already tried in Phase 2 (extension-matched).
    let tried_names: Vec<&str> = ext_codecs.iter().map(|c| c.name).collect();
    for codec in &magic_codecs {
      if tried_names.contains(&codec.name) {
        continue;
      }
      let reader = source.open_seekable()?;
      match (codec.open_fn)(reader) {
        Ok(stream) => return Ok((stream, codec.name)),
        Err(_) => continue,
      }
    }

    // ---- Phase 5: Try ALL remaining codecs ----
    // Everything not yet tried, in registration order.
    let all_tried: Vec<&str> = tried_names
      .into_iter()
      .chain(magic_codecs.iter().map(|c| c.name))
      .collect();
    for codec in registry.all() {
      if all_tried.contains(&codec.name) {
        continue;
      }
      let reader = source.open_seekable()?;
      match (codec.open_fn)(reader) {
        Ok(stream) => return Ok((stream, codec.name)),
        Err(_) => continue,
      }
    }

    Err(AroundError::UnsupportedFormat {
      format: ext,
      reason: "no codec could open the source".into(),
    })
  }

  /// Play a source. Blocks until playback completes or stop/pause/seek is signaled.
  pub fn play(
    &self,
    source: Box<dyn Source>,
    state: Arc<Mutex<PlaybackState>>,
  ) -> Result<PlaybackHandle, AroundError> {
    let seekable = source.capabilities().contains(SourceCapabilities::SEEKABLE);

    let (mut audio_stream, codec_name) =
      Self::open_codec(&self.registry, source.as_ref(), seekable)?;

    let sample_rate = audio_stream.sample_rate as u64;
    // Guard against buggy codecs returning sample_rate=0 (would panic on division).
    if sample_rate == 0 {
      return Err(AroundError::DecodeError {
        message: "codec returned sample_rate=0".into(),
      });
    }
    let output_format =
      around_core::SampleSpec::new(audio_stream.sample_rate, audio_stream.channels, 16).unwrap_or(
        around_core::SampleSpec {
          sample_rate: audio_stream.sample_rate,
          channels: audio_stream.channels,
          bit_depth: 16,
        },
      );

    let duration_ms = if audio_stream.total_frames > 0 {
      Some((audio_stream.total_frames as u128 * 1000 / sample_rate as u128) as u64)
    } else {
      None
    };

    {
      let mut st = state.lock().unwrap();
      st.state = "buffering".into();
      st.playing = true;
      st.position_ms = 0;
      st.output_format = Some(output_format);
      st.format_name = Some(codec_name.to_string());
      st.seekable = seekable;
      st.duration_ms = duration_ms;
    }

    let running = Arc::clone(&self.running);
    running.store(true, Ordering::SeqCst);
    self.paused.store(false, Ordering::SeqCst);
    *self.seek_target_ms.lock().unwrap() = None;

    let output = Arc::new(AudioOutput::new(4));
    let output_sender = output.sender_clone();

    let host = cpal::default_host();
    let device = host
      .default_output_device()
      .ok_or_else(|| AroundError::Internal {
        message: "no audio output device found".into(),
      })?;
    let config = device
      .default_output_config()
      .map_err(|e| AroundError::Internal {
        message: format!("audio device error: {}", e),
      })?;

    let output_cb = Arc::clone(&output);
    let device_lost_flag = Arc::clone(&self.device_lost);
    let cpal_stream = device
      .build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
          let mut written = 0;
          while written < data.len() {
            if let Some(chunk) = output_cb.try_recv() {
              let to_copy = chunk.len().min(data.len() - written);
              data[written..written + to_copy].copy_from_slice(&chunk[..to_copy]);
              written += to_copy;
            } else {
              data[written..].fill(0.0);
              break;
            }
          }
        },
        move |err| {
          tracing::error!(?err, "cpal stream error");
          device_lost_flag.store(true, Ordering::SeqCst);
        },
        None,
      )
      .map_err(|e| AroundError::Internal {
        message: format!("failed to build audio output stream: {}", e),
      })?;

    cpal_stream.play().map_err(|e| AroundError::Internal {
      message: format!("failed to start audio output stream: {}", e),
    })?;

    let _span = tracing::info_span!("playback").entered();
    tracing::info!(codec = codec_name, "starting playback");

    {
      let mut st = state.lock().unwrap();
      st.state = "playing".into();
    }

    let max_ch = output_format.channels.max(1) as usize;
    let mut buf = vec![0.0f32; 4096 * max_ch];
    let mut total_frames = 0usize;
    let mut consecutive_errors = 0u32;

    while running.load(Ordering::SeqCst) {
      if self.device_lost.load(Ordering::SeqCst) {
        tracing::warn!("playback paused: audio device lost");
        {
          let mut st = state.lock().unwrap();
          st.device_lost = true;
        }
        if self.config.output_auto_reconnect {
          self.device_lost.store(false, Ordering::SeqCst);
          continue;
        } else {
          break;
        }
      }

      if self.paused.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(10));
        continue;
      }

      if seekable {
        if let Some(target_ms) = self.seek_target_ms.lock().unwrap().take() {
          let frame = target_ms * sample_rate / 1000;
          if let Err(e) = audio_stream.seek(frame) {
            tracing::warn!(?e, target_ms, "seek failed, continuing");
          } else {
            total_frames = frame as usize;
            let pos = total_frames as u64 * 1000 / sample_rate;
            let mut st = state.lock().unwrap();
            st.position_ms = pos;
            st.state = "playing".into();
          }
        }
      } else {
        let _ = self.seek_target_ms.lock().unwrap().take();
      }

      match audio_stream.read(&mut buf) {
        Ok(Some(n)) => {
          let sample_count = n * output_format.channels as usize;
          let samples = buf[..sample_count].to_vec();
          if output_sender.send(samples).is_err() {
            tracing::warn!("audio output disconnected, stopping");
            break;
          }
          total_frames += n;
          consecutive_errors = 0;
          let position_ms = total_frames as u64 * 1000 / sample_rate;
          let mut st = state.lock().unwrap();
          st.position_ms = position_ms;
        }
        Ok(None) => break,
        Err(e) => {
          consecutive_errors += 1;
          tracing::warn!(?e, consecutive_errors, "decode error during playback");
          if consecutive_errors >= 3 {
            tracing::error!(?e, "too many consecutive decode errors, stopping");
            {
              let mut st = state.lock().unwrap();
              st.state = "error".into();
              st.playing = false;
            }
            return Err(e);
          }
        }
      }
    }

    tracing::info!(total_frames, "playback complete");
    running.store(false, Ordering::SeqCst);

    {
      let mut st = state.lock().unwrap();
      st.state = "stopped".into();
      st.playing = false;
      st.position_ms = total_frames as u64 * 1000 / sample_rate;
    }

    Ok(PlaybackHandle {
      output_format,
      running: Arc::clone(&self.running),
    })
  }

  pub fn config(&self) -> &crate::config::EngineConfig {
    &self.config
  }
  pub fn stop(&self) {
    self.running.store(false, Ordering::SeqCst);
  }
  pub fn pause(&self) {
    self.paused.store(true, Ordering::SeqCst);
  }
  pub fn resume(&self) {
    self.paused.store(false, Ordering::SeqCst);
  }
  pub fn seek(&self, position_ms: u64) {
    *self.seek_target_ms.lock().unwrap() = Some(position_ms);
  }
  pub fn signal_shutdown(&self) {
    self.shutdown.store(true, Ordering::SeqCst);
  }
  pub fn is_shutdown(&self) -> bool {
    self.shutdown.load(Ordering::SeqCst)
  }
  pub fn notify_device_lost(&self) {
    self.device_lost.store(true, Ordering::SeqCst);
    tracing::warn!("audio output device disconnected");
    if !self.config.output_auto_reconnect {
      self.stop();
    }
  }
  pub fn is_device_lost(&self) -> bool {
    self.device_lost.load(Ordering::SeqCst)
  }
}

fn same_codec_set(a: &[&around_core::CodecInfo], b: &[&around_core::CodecInfo]) -> bool {
  if a.len() != b.len() {
    return false;
  }
  a.iter().all(|ca| b.iter().any(|cb| std::ptr::eq(*ca, *cb)))
}

#[cfg(test)]
mod tests {
  use super::*;
  use around_core::FormatSignature;

  fn make_codec(
    name: &'static str,
    ext: &'static str,
    magic: &'static [u8],
  ) -> around_core::CodecInfo {
    let fmts: &'static [FormatSignature] =
      Box::leak(Box::new([
        FormatSignature::from_extension(ext, "test").with_magic(magic)
      ]));
    around_core::CodecInfo::new(name, fmts, |_| unreachable!())
  }

  #[test]
  fn same_codec_set_empty() {
    assert!(same_codec_set(&[], &[]));
  }

  #[test]
  fn same_codec_set_single_identical() {
    let a = make_codec("wav", "wav", b"RIFF");
    assert!(same_codec_set(&[&a], &[&a]));
  }

  #[test]
  fn same_codec_set_different_length() {
    let a = make_codec("wav", "wav", b"RIFF");
    let b = make_codec("mp3", "mp3", b"ID3");
    assert!(!same_codec_set(&[&a], &[&a, &b]));
  }

  #[test]
  fn same_codec_set_different_items() {
    let a = make_codec("wav", "wav", b"RIFF");
    let b = make_codec("mp3", "mp3", b"ID3");
    assert!(!same_codec_set(&[&a], &[&b]));
  }

  #[test]
  fn same_codec_set_same_items_different_order() {
    let a = make_codec("wav", "wav", b"RIFF");
    let b = make_codec("mp3", "mp3", b"ID3");
    assert!(same_codec_set(&[&a, &b], &[&b, &a]));
  }

  #[test]
  fn same_codec_set_pointer_identity() {
    // Two distinct CodecInfo values with same fields are NOT the same
    // (same_codec_set uses pointer identity, not structural equality).
    let a1 = make_codec("wav", "wav", b"RIFF");
    let a2 = make_codec("wav", "wav", b"RIFF");
    assert!(!same_codec_set(&[&a1], &[&a2]));
    // But same pointer IS same
    assert!(same_codec_set(&[&a1], &[&a1]));
  }
}

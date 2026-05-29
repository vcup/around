//! Audio output ring buffer: decouples the decode thread from the cpal callback.

use crossbeam::channel::{self, Receiver, Sender, TryRecvError};

/// Wraps a crossbeam SPSC channel for passing f32 PCM samples
/// from the decode thread (producer) to the cpal callback (consumer).
pub struct AudioOutput {
  sender: Sender<Vec<f32>>,
  receiver: Receiver<Vec<f32>>,
}

impl AudioOutput {
  /// Create a new audio output ring buffer with the given capacity (in chunks).
  pub fn new(capacity: usize) -> Self {
    let (sender, receiver) = channel::bounded(capacity);
    Self { sender, receiver }
  }

  /// Send a chunk of interleaved f32 PCM samples from the decode thread.
  /// Blocks if the buffer is full (backpressure — decode loop waits for audio to consume).
  pub fn send(&self, samples: Vec<f32>) -> Result<(), crossbeam::channel::SendError<Vec<f32>>> {
    self.sender.send(samples)
  }

  /// Called from the cpal callback on the real-time audio thread.
  /// Returns samples if available, or None on underrun (caller should write silence).
  /// MUST NOT block — uses try_recv.
  pub fn try_recv(&self) -> Option<Vec<f32>> {
    match self.receiver.try_recv() {
      Ok(samples) => Some(samples),
      Err(TryRecvError::Empty) => None,
      Err(TryRecvError::Disconnected) => None,
    }
  }

  /// Clone the sender handle for sharing with the decode thread.
  pub fn sender_clone(&self) -> Sender<Vec<f32>> {
    self.sender.clone()
  }
}

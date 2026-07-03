//! Audio output ring buffer: decouples the decode thread from the cpal callback.
//!
//! Uses `ringbuf::HeapRb<f32>` — a lock-free SPSC ring buffer (ADR-0006 §4).

use ringbuf::{traits::*, HeapCons, HeapProd, HeapRb};

/// Producer handle — send PCM samples from decode threads.
pub struct AudioProducer {
  inner: HeapProd<f32>,
}

impl AudioProducer {
  pub fn push_slice(&mut self, samples: &[f32]) -> usize {
    self.inner.push_slice(samples)
  }
}

/// Consumer handle — read PCM samples from the CPAL callback.
pub struct AudioConsumer {
  inner: HeapCons<f32>,
}

impl AudioConsumer {
  pub fn pop_slice(&mut self, buf: &mut [f32]) -> usize {
    self.inner.pop_slice(buf)
  }

  pub fn is_empty(&self) -> bool {
    self.inner.is_empty()
  }
}

/// Create a new producer/consumer pair backed by a shared ring buffer.
pub fn create_output(capacity: usize) -> (AudioProducer, AudioConsumer) {
  let rb = HeapRb::new(capacity);
  let (prod, cons) = rb.split();
  (AudioProducer { inner: prod }, AudioConsumer { inner: cons })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_round_trip() {
    let (mut prod, mut cons) = create_output(1024);
    let samples: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
    let pushed = prod.push_slice(&samples);
    assert_eq!(pushed, 100);

    let mut buf = vec![0.0f32; 200];
    let popped = cons.pop_slice(&mut buf);
    assert_eq!(popped, 100);
    assert_eq!(&buf[..100], &samples[..]);
  }

  #[test]
  fn test_empty_returns_zero() {
    let (_, mut cons) = create_output(1024);
    let mut buf = [0.0f32; 64];
    assert_eq!(cons.pop_slice(&mut buf), 0);
  }
}

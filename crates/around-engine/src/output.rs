//! Byte output ring used by OutputBinding callbacks.

use ringbuf::{traits::*, HeapCons, HeapProd, HeapRb};

pub struct AudioProducer {
  inner: HeapProd<u8>,
}

impl AudioProducer {
  pub fn push_slice(&mut self, bytes: &[u8]) -> usize {
    self.inner.push_slice(bytes)
  }
}

pub struct AudioConsumer {
  inner: HeapCons<u8>,
}

impl AudioConsumer {
  pub fn pop_slice(&mut self, bytes: &mut [u8]) -> usize {
    self.inner.pop_slice(bytes)
  }
}

pub fn create_output(capacity: usize) -> (AudioProducer, AudioConsumer) {
  let rb = HeapRb::new(capacity);
  let (prod, cons) = rb.split();
  (AudioProducer { inner: prod }, AudioConsumer { inner: cons })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn round_trip_bytes() {
    let (mut prod, mut cons) = create_output(1024);
    let bytes: Vec<u8> = (0..100).collect();
    assert_eq!(prod.push_slice(&bytes), 100);
    let mut out = vec![0; 200];
    assert_eq!(cons.pop_slice(&mut out), 100);
    assert_eq!(&out[..100], &bytes[..]);
  }

  #[test]
  fn empty_returns_zero() {
    let (_, mut cons) = create_output(1024);
    assert_eq!(cons.pop_slice(&mut [0; 64]), 0);
  }
}

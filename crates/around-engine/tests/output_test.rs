use around_engine::output::AudioOutput;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn test_send_and_receive() {
  let output = AudioOutput::new(4);

  output.send(vec![1.0f32, 2.0]).unwrap();
  output.send(vec![3.0f32, 4.0]).unwrap();

  let chunk1 = output.try_recv();
  assert_eq!(chunk1, Some(vec![1.0f32, 2.0]));

  let chunk2 = output.try_recv();
  assert_eq!(chunk2, Some(vec![3.0f32, 4.0]));
}

#[test]
fn test_underrun_returns_none() {
  let output = AudioOutput::new(1);
  let result = output.try_recv();
  assert!(result.is_none());
}

#[test]
fn test_sender_clone() {
  let output = AudioOutput::new(4);

  let sender1 = output.sender_clone();
  let sender2 = output.sender_clone();

  sender1.send(vec![1.0f32, 2.0]).unwrap();
  sender2.send(vec![3.0f32, 4.0]).unwrap();

  let chunk1 = output.try_recv();
  assert_eq!(chunk1, Some(vec![1.0f32, 2.0]));

  let chunk2 = output.try_recv();
  assert_eq!(chunk2, Some(vec![3.0f32, 4.0]));
}

#[test]
fn test_backpressure() {
  let output = Arc::new(AudioOutput::new(1));

  // Fill the capacity-1 buffer (non-blocking: buffer starts empty)
  output.send(vec![1.0f32]).unwrap();

  let output_clone = output.clone();
  let received = std::thread::spawn(move || {
    // Wait to give the main thread time to block on send
    std::thread::sleep(Duration::from_millis(50));
    output_clone.try_recv()
  });

  // This send blocks until the spawned thread frees the slot
  output.send(vec![2.0f32]).unwrap();

  let first = received.join().unwrap();
  assert_eq!(first, Some(vec![1.0f32]));

  let second = output.try_recv();
  assert_eq!(second, Some(vec![2.0f32]));
}

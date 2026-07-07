//! Metadata types for audio tracks.

use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
  pub title: Option<String>,
  pub artist: Option<String>,
  pub album: Option<String>,
  pub duration: Option<Duration>,
  pub genre: Option<String>,
}

impl Metadata {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn with_title(mut self, title: impl Into<String>) -> Self {
    self.title = Some(title.into());
    self
  }

  pub fn with_artist(mut self, artist: impl Into<String>) -> Self {
    self.artist = Some(artist.into());
    self
  }

  pub fn with_album(mut self, album: impl Into<String>) -> Self {
    self.album = Some(album.into());
    self
  }

  pub fn with_duration(mut self, duration: Duration) -> Self {
    self.duration = Some(duration);
    self
  }

  pub fn with_genre(mut self, genre: impl Into<String>) -> Self {
    self.genre = Some(genre.into());
    self
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn metadata_default_is_empty() {
    let m = Metadata::default();
    assert_eq!(m.title, None);
    assert_eq!(m.artist, None);
    assert_eq!(m.album, None);
    assert_eq!(m.duration, None);
    assert_eq!(m.genre, None);
  }

  #[test]
  fn metadata_new_is_empty() {
    let m = Metadata::new();
    assert_eq!(m.title, None);
    assert_eq!(m.artist, None);
    assert_eq!(m.album, None);
    assert_eq!(m.duration, None);
  }

  #[test]
  fn metadata_with_title_sets_title() {
    let m = Metadata::new().with_title("Test Song");
    assert_eq!(m.title, Some("Test Song".to_string()));
  }

  #[test]
  fn metadata_with_artist_sets_artist() {
    let m = Metadata::new().with_artist("Test Artist");
    assert_eq!(m.artist, Some("Test Artist".to_string()));
  }

  #[test]
  fn metadata_with_album_sets_album() {
    let m = Metadata::new().with_album("Test Album");
    assert_eq!(m.album, Some("Test Album".to_string()));
  }

  #[test]
  fn metadata_with_duration_sets_duration() {
    let m = Metadata::new().with_duration(Duration::from_secs(180));
    assert_eq!(m.duration, Some(Duration::from_secs(180)));
  }

  #[test]
  fn metadata_with_genre_sets_genre() {
    let m = Metadata::new().with_genre("Jazz");
    assert_eq!(m.genre, Some("Jazz".to_string()));
  }

  #[test]
  fn metadata_chained_builders() {
    let m = Metadata::new()
      .with_title("Song")
      .with_artist("Artist")
      .with_album("Album")
      .with_duration(Duration::from_secs(120))
      .with_genre("Rock");
    assert_eq!(m.title, Some("Song".to_string()));
    assert_eq!(m.artist, Some("Artist".to_string()));
    assert_eq!(m.album, Some("Album".to_string()));
    assert_eq!(m.duration, Some(Duration::from_secs(120)));
    assert_eq!(m.genre, Some("Rock".to_string()));
  }
}

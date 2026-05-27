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

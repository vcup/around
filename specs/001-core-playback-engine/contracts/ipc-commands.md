# Contract: IPC Command Protocol

**Version**: 0.1.0-pre
**Transport**: Unix domain socket (Linux/macOS), named pipe (Windows)
**Encoding**: JSON, one command/response per newline-delimited message

## Command Schema

### Play

```json
{"command": "play", "path": "/path/to/song.wav"}
→ {"status": "ok", "track_id": 1}
→ {"status": "error", "code": "FILE_NOT_FOUND", "message": "No such file"}
```

### Pause

```json
{"command": "pause"}
→ {"status": "ok", "state": "paused", "position_ms": 12345}
```

### Resume

```json
{"command": "resume"}
→ {"status": "ok", "state": "playing", "position_ms": 12345}
```

### Seek

```json
{"command": "seek", "position_ms": 30000}
→ {"status": "ok", "position_ms": 30000}
→ {"status": "error", "code": "INVALID_POSITION", "message": "Position exceeds duration"}
```

### Stop

```json
{"command": "stop"}
→ {"status": "ok", "state": "stopped"}
```

### Status

```json
{"command": "status"}
→ {
    "status": "ok",
    "state": "playing",
    "track": {
      "id": 1,
      "path": "/path/to/song.wav",
      "format": "WAV",
      "duration_ms": 245000
    },
    "position_ms": 12345,
    "volume": 1.0
  }
```

### Load Decoder

Two variants support different permission models.

**Path-based** (`load_decoder`) — engine reads the file directly using its own
filesystem permissions. Use when engine and CLI share filesystem access
(same machine, same user, same container).

```json
{"command": "load_decoder", "path": "/path/to/libcodec.so"}
→ {"status": "ok", "name": "ffmpeg-codec", "formats": ["MP3", "AAC", "FLAC", "Opus"]}
→ {"status": "error", "code": "DECODER_LOAD_FAILED", "message": "Not a valid shared library"}
```

**Bytes-based** (`load_decoder_bytes`) — CLI reads the file and sends raw bytes.
Engine writes to a temp file, loads via `libloading`, then deletes the temp.
Use when CLI has filesystem access but engine does not (different user, container
without host mount, binary fetched over network).

```json
{"command": "load_decoder_bytes", "data": "<base64-encoded bytes>", "name": "ffmpeg-codec"}
→ {"status": "ok", "name": "ffmpeg-codec", "formats": ["MP3", "AAC", "FLAC", "Opus"]}
→ {"status": "error", "code": "DECODER_LOAD_FAILED", "message": "Binary is not a valid shared library"}
```


```json
{"command": "list_decoders"}
→ {
    "status": "ok",
    "decoders": [
      {"name": "wav-builtin", "formats": ["WAV"], "source": "builtin"},
      {"name": "ffmpeg-codec", "formats": ["MP3", "AAC", "FLAC", "Opus"], "source": "discovered", "path": "~/.local/share/around/decoders/libaround_codec_ffmpeg.so"},
      {"name": "custom-decoder", "formats": ["QOA"], "source": "bytes"}
    ]
  }
```

## Error Codes

| Code | Meaning |
|------|---------|
| `FILE_NOT_FOUND` | Source path does not exist |
| `UNSUPPORTED_FORMAT` | No decoder registered for detected format |
| `DECODE_ERROR` | Decoder failed during playback |
| `INVALID_POSITION` | Seek position beyond duration or negative |
| `NO_TRACK` | Command requires active playback but none exists |
| `DECODER_LOAD_FAILED` | Extension shared library failed to load |
| `INTERNAL` | Unexpected engine error |

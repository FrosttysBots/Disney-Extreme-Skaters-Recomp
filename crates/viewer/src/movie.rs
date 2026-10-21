//! The game's movies (`movies/*.bik`: the logos, the intro, the characters'
//! videos), played through ffmpeg.
//!
//! They're Bink 1 video (revision `i`, 640x480 at 29.97 frames a second)
//! with Bink DCT audio. Nothing here decodes Bink: the movie's bytes are
//! streamed off the disc into two `ffmpeg` processes, one handing back
//! the frames as RGBA and the other the sound as 32-bit float stereo
//! samples. Without ffmpeg (on the `PATH`, or named by `DESA_FFMPEG`) the
//! movies can't be played.
//!
//! The Bink header, read here for the size and frame rate:
//!
//! ```text
//! 0   "BIK" + revision letter
//! 8   u32 frames
//! 20  u32 width, u32 height
//! 28  u32 frame rate numerator, u32 denominator
//! 40  u32 audio tracks
//! ```
//! (little-endian, unlike the rest of the disc.)

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread;

use anyhow::{Context, Result, bail};

/// Where a movie's bytes are: a file (the disc image, or the movie itself
/// in an extracted folder) and the range of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovieSource {
    pub path: PathBuf,
    pub offset: u64,
    pub size: u64,
}

impl MovieSource {
    /// The first `n` bytes.
    fn head(&self, n: usize) -> Result<Vec<u8>> {
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = vec![0; n.min(self.size as usize)];
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Streams the movie's bytes into `out` till it's all sent or `out`
    /// closes (the movie stopped).
    fn feed(&self, mut out: impl Write) {
        let Ok(mut file) = File::open(&self.path) else {
            return;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut left = self.size;
        let mut buf = vec![0; 1 << 16];
        while left > 0 {
            let n = buf.len().min(left as usize);
            if file.read_exact(&mut buf[..n]).is_err() || out.write_all(&buf[..n]).is_err() {
                return;
            }
            left -= n as u64;
        }
    }
}

/// A Bink movie's size and timing, from its header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BinkInfo {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps: f64,
    pub audio_tracks: u32,
}

impl BinkInfo {
    pub fn parse(header: &[u8]) -> Result<Self> {
        if header.len() < 44 || &header[..3] != b"BIK" {
            bail!("not a Bink movie");
        }
        let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        let (num, den) = (u32_at(28), u32_at(32));
        Ok(BinkInfo {
            frames: u32_at(8),
            width: u32_at(20),
            height: u32_at(24),
            fps: if den == 0 {
                30.0
            } else {
                f64::from(num) / f64::from(den)
            },
            audio_tracks: u32_at(40),
        })
    }

    pub fn duration(&self) -> f64 {
        f64::from(self.frames) / self.fps
    }
}

/// The ffmpeg to run: `DESA_FFMPEG`, else `ffmpeg` on the `PATH`.
pub fn ffmpeg() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DESA_FFMPEG") {
        return Some(PathBuf::from(path));
    }
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

/// The audio's sample rate and channels, as ffmpeg's asked to give it.
pub const AUDIO_RATE: u32 = 44_100;
pub const AUDIO_CHANNELS: u16 = 2;

/// A movie playing: frames come in from ffmpeg as fast as it decodes
/// them (a few ahead), and are taken by the clock.
pub struct Movie {
    pub info: BinkInfo,
    frames: Receiver<Vec<u8>>,
    /// The frame shown now and its number.
    current: Option<(u64, Vec<u8>)>,
    /// Frames taken so far (the next one's number).
    taken: u64,
    finished: bool,
    children: Vec<Child>,
}

impl Movie {
    /// Starts a movie playing: its frames, and its sound if `audio` (the
    /// samples, interleaved stereo, sent as they're decoded).
    pub fn start(
        source: &MovieSource,
        ffmpeg: &Path,
    ) -> Result<(Movie, Option<Receiver<Vec<f32>>>)> {
        let info = BinkInfo::parse(&source.head(44)?)?;
        let frame_bytes = (info.width * info.height * 4) as usize;
        let mut children = Vec::new();

        // The pictures.
        let mut video = spawn(ffmpeg, &["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])?;
        feed(source, &mut video);
        let mut out = video.stdout.take().context("no ffmpeg output")?;
        let (send, frames) = sync_channel::<Vec<u8>>(6);
        thread::spawn(move || {
            loop {
                let mut frame = vec![0; frame_bytes];
                if out.read_exact(&mut frame).is_err() || send.send(frame).is_err() {
                    return;
                }
            }
        });
        children.push(video);

        // The sound.
        let mut audio_rx = None;
        if info.audio_tracks > 0 {
            let rate = AUDIO_RATE.to_string();
            let channels = AUDIO_CHANNELS.to_string();
            let args = [
                "-vn", "-f", "f32le", "-ac", &channels, "-ar", &rate, "pipe:1",
            ];
            if let Ok(mut audio) = spawn(ffmpeg, &args) {
                feed(source, &mut audio);
                if let Some(mut out) = audio.stdout.take() {
                    let (send, recv): (SyncSender<Vec<f32>>, _) = sync_channel(64);
                    thread::spawn(move || {
                        let mut buf = vec![0u8; 4096 * 4];
                        loop {
                            let Ok(n) = read_some(&mut out, &mut buf) else {
                                return;
                            };
                            if n == 0 {
                                return;
                            }
                            let samples = buf[..n]
                                .chunks_exact(4)
                                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                                .collect();
                            if send.send(samples).is_err() {
                                return;
                            }
                        }
                    });
                    audio_rx = Some(recv);
                }
                children.push(audio);
            }
        }
        Ok((
            Movie {
                info,
                frames,
                current: None,
                taken: 0,
                finished: false,
                children,
            },
            audio_rx,
        ))
    }

    /// The frame to show `seconds` in (RGBA, `info.width` by
    /// `info.height`), and whether it's new since last asked.
    pub fn frame_at(&mut self, seconds: f64) -> Option<(&[u8], bool)> {
        let want = (seconds * self.info.fps).floor().max(0.0) as u64;
        let mut fresh = false;
        while self.taken <= want {
            match self.frames.try_recv() {
                Ok(frame) => {
                    self.current = Some((self.taken, frame));
                    self.taken += 1;
                    fresh = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        self.current.as_ref().map(|(_, f)| (f.as_slice(), fresh))
    }

    /// Whether every frame's been shown.
    pub fn finished(&self) -> bool {
        self.finished
    }
}

impl Drop for Movie {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Reads what's there (at least a byte, unless at the end).
fn read_some(from: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    // Whole samples: keep reading till a multiple of 4 or the end.
    let mut n = from.read(buf)?;
    while n % 4 != 0 {
        let more = from.read(&mut buf[n..])?;
        if more == 0 {
            break;
        }
        n += more;
    }
    Ok(n - n % 4)
}

/// ffmpeg reading a Bink movie from its input and writing as `args` say.
fn spawn(ffmpeg: &Path, args: &[&str]) -> Result<Child> {
    let mut command = Command::new(ffmpeg);
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "bink",
            "-i",
            "pipe:0",
        ])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        command.creation_flags(0x0800_0000);
    }
    command
        .spawn()
        .with_context(|| format!("couldn't run {}", ffmpeg.display()))
}

/// Streams the movie into the process's input on a thread of its own.
fn feed(source: &MovieSource, child: &mut Child) {
    if let Some(stdin) = child.stdin.take() {
        let source = source.clone();
        thread::spawn(move || source.feed(stdin));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_bink_header() {
        // intro.bik's first 44 bytes.
        let header = [
            0x42, 0x49, 0x4b, 0x69, 0x84, 0x82, 0x10, 0x05, 0x1d, 0x0d, 0x00, 0x00, 0x44, 0x68,
            0x01, 0x00, 0x1d, 0x0d, 0x00, 0x00, 0x80, 0x02, 0x00, 0x00, 0xe0, 0x01, 0x00, 0x00,
            0xb5, 0x0b, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x00, 0x00,
        ];
        let info = BinkInfo::parse(&header).unwrap();
        assert_eq!((info.width, info.height, info.frames), (640, 480, 3357));
        assert!((info.fps - 29.97).abs() < 1e-9);
        assert_eq!(info.audio_tracks, 1);
        assert!((info.duration() - 112.01).abs() < 0.01);
    }
}

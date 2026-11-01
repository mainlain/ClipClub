//! Fail-closed media validation. Network metadata is never considered proof of a valid video.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_duration_secs: f64,
    pub max_width: u32,
    pub max_height: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: crate::core::MAX_FILE_BYTES,
            max_duration_secs: crate::core::MAX_DURATION_SECONDS,
            max_width: crate::core::MAX_DIMENSION,
            max_height: crate::core::MAX_DIMENSION,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidatedVideo {
    pub id: String,
    pub extension: String,
    pub size: u64,
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
}

/// Release builds embed the probe, so moving the application needs no installed FFmpeg.
/// Development builds deliberately fail closed when no probe is available.
pub fn install_probe(data_root: &Path) -> Result<PathBuf> {
    #[cfg(feature = "bundle-ffprobe")]
    {
        const PROBE: &[u8] = include_bytes!(env!("CLIPCROP_FFPROBE"));
        let directory = data_root.join("bin");
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let executable = directory.join(if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        });
        let existing_matches = std::fs::read(&executable)
            .map(|b| Sha256::digest(&b) == Sha256::digest(PROBE))
            .unwrap_or(false);
        if !existing_matches {
            let temporary = directory.join("ffprobe.installing");
            let _ = std::fs::remove_file(&temporary);
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o700);
            }
            use std::io::Write;
            let mut file = options.open(&temporary)?;
            file.write_all(PROBE)?;
            file.sync_all()?;
            drop(file);
            #[cfg(windows)]
            if executable.exists() {
                std::fs::remove_file(&executable)?;
            }
            std::fs::rename(temporary, &executable)?;
        }
        Ok(executable)
    }
    #[cfg(not(feature = "bundle-ffprobe"))]
    {
        let _ = data_root;
        if let Some(path) = std::env::var_os("CLIPCROP_FFPROBE") {
            let path = PathBuf::from(path);
            ensure!(
                path.is_file(),
                "CLIPCROP_FFPROBE must point to an ffprobe executable"
            );
            return Ok(path);
        }
        if let Ok(executable) = std::env::current_exe() {
            if let Some(parent) = executable.parent() {
                let path = parent.join(if cfg!(windows) {
                    "ffprobe.exe"
                } else {
                    "ffprobe"
                });
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
        for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            let path = directory.join(if cfg!(windows) {
                "ffprobe.exe"
            } else {
                "ffprobe"
            });
            if path.is_file() {
                return Ok(path);
            }
        }
        bail!("Video validation needs ffprobe. Use a bundled release or set CLIPCROP_FFPROBE.");
    }
}

pub fn check_header(bytes: &[u8], extension: &str, limits: &Limits) -> Result<()> {
    ensure!(!bytes.is_empty(), "The video is empty");
    ensure!(
        bytes.len() as u64 <= limits.max_bytes,
        "Video exceeds the {} byte limit",
        limits.max_bytes
    );
    match extension {
        "mp4" => ensure!(
            bytes.len() >= 12 && &bytes[4..8] == b"ftyp",
            "The file does not have an MP4 container signature"
        ),
        "webm" => {
            ensure!(
                bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
                "The file does not have a WebM container signature"
            );
            ensure!(
                bytes[..bytes.len().min(4096)]
                    .windows(4)
                    .any(|w| w == b"webm"),
                "Matroska files must use the WebM document type"
            );
        }
        _ => bail!("Only MP4 and WebM videos are allowed"),
    }
    Ok(())
}

/// Call on a private immutable snapshot, rather than a user-controlled original path.
pub async fn validate_video(path: &Path, limits: &Limits, probe: &Path) -> Result<ValidatedVideo> {
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .context("Cannot read video")?;
    ensure!(
        metadata.file_type().is_file(),
        "Only regular files are allowed"
    );
    ensure!(
        metadata.len() > 0 && metadata.len() <= limits.max_bytes,
        "Video must be between 1 and {} bytes",
        limits.max_bytes
    );
    let mut bytes = Vec::new();
    tokio::fs::File::open(path)
        .await?
        .take(limits.max_bytes + 1)
        .read_to_end(&mut bytes)
        .await?;
    check_header(&bytes, &extension, limits)?;
    let id = hex::encode(Sha256::digest(&bytes));
    let absolute = tokio::fs::canonicalize(path).await?;
    let mut command = Command::new(probe);
    // No shell is involved. Only the file protocol and the two supported demuxers may run.
    // No drefs are enabled; MOV external data references remain disabled by FFmpeg's default.
    let probe_size = limits.max_bytes.to_string();
    let analyze_duration = (((limits.max_duration_secs + 1.0) * 1_000_000.0) as u64).to_string();
    command.args([
        "-v", "error", "-max_alloc", "33554432", "-threads", "1",
        "-protocol_whitelist", "file", "-format_whitelist", "mov,matroska,webm",
        "-probesize", probe_size.as_str(), "-analyzeduration", analyze_duration.as_str(), "-err_detect", "explode",
        "-show_streams", "-show_format", "-show_frames", "-show_entries",
        "format=format_name,duration:stream=codec_type,codec_name,width,height,coded_width,coded_height,duration:stream_disposition=attached_pic:frame=media_type,width,height,best_effort_timestamp_time,duration_time,pkt_duration_time",
        "-of", "json", "-i",
    ]).arg(&absolute).env_remove("FFREPORT").env("AV_LOG_FORCE_NOCOLOR", "1")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .context("Could not start the video validator")?;
    let stdout = child.stdout.take().context("Validator stdout missing")?;
    let stderr = child.stderr.take().context("Validator stderr missing")?;
    const MAX_OUTPUT: u64 = 8_000_000;
    let scan = async {
        let read_stdout = async {
            let mut output = Vec::new();
            stdout.take(MAX_OUTPUT + 1).read_to_end(&mut output).await?;
            ensure!(
                output.len() as u64 <= MAX_OUTPUT,
                "Video has excessive frame metadata"
            );
            Ok::<_, anyhow::Error>(output)
        };
        let read_stderr = async {
            let mut output = Vec::new();
            stderr.take(32_769).read_to_end(&mut output).await?;
            ensure!(
                output.len() <= 32_768,
                "Video decoder produced excessive errors"
            );
            Ok::<_, anyhow::Error>(output)
        };
        let (output, errors) = tokio::try_join!(read_stdout, read_stderr)?;
        let status = child.wait().await?;
        ensure!(
            status.success() && errors.is_empty(),
            "Video is malformed or could not be decoded safely"
        );
        Ok::<_, anyhow::Error>(output)
    };
    let output = match tokio::time::timeout(Duration::from_secs(45), scan).await {
        Ok(Ok(output)) => output,
        result => {
            let _ = child.kill().await;
            match result {
                Ok(Err(error)) => return Err(error),
                _ => bail!("Video validation timed out"),
            }
        }
    };
    let report: serde_json::Value =
        serde_json::from_slice(&output).context("Invalid validator response")?;
    let (duration_secs, width, height) = check_report(&report, &extension, limits)?;
    // Detect accidental mutation even when this API is misused with a mutable source file.
    let mut after = Vec::new();
    tokio::fs::File::open(path)
        .await?
        .take(limits.max_bytes + 1)
        .read_to_end(&mut after)
        .await?;
    ensure!(
        after.len() == bytes.len() && Sha256::digest(&after) == Sha256::digest(&bytes),
        "Video changed during validation"
    );
    Ok(ValidatedVideo {
        id,
        extension,
        size: bytes.len() as u64,
        duration_secs,
        width,
        height,
    })
}

fn seconds(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n: &f64| n.is_finite())
}

fn dimensions(value: &serde_json::Value, limits: &Limits) -> Result<(u32, u32)> {
    let width = value["width"].as_u64().context("Video width is missing")?;
    let height = value["height"]
        .as_u64()
        .context("Video height is missing")?;
    ensure!(
        width > 0
            && height > 0
            && width <= limits.max_width as u64
            && height <= limits.max_height as u64,
        "Video resolution exceeds {} × {} pixels",
        limits.max_width,
        limits.max_height
    );
    for (key, maximum) in [
        ("coded_width", limits.max_width),
        ("coded_height", limits.max_height),
    ] {
        if let Some(dimension) = value[key].as_u64() {
            ensure!(
                dimension <= maximum as u64,
                "Coded video resolution exceeds the limit"
            );
        }
    }
    Ok((width as u32, height as u32))
}

fn check_report(
    report: &serde_json::Value,
    extension: &str,
    limits: &Limits,
) -> Result<(f64, u32, u32)> {
    let format = report["format"]["format_name"]
        .as_str()
        .context("Container is missing")?;
    ensure!(
        match extension {
            "mp4" => format.split(',').any(|f| f == "mp4" || f == "mov"),
            "webm" => format.split(',').any(|f| f == "webm"),
            _ => false,
        },
        "Container does not match the file extension"
    );
    let duration =
        seconds(&report["format"]["duration"]).context("Video duration must be known")?;
    ensure!(
        duration > 0.0 && duration <= limits.max_duration_secs,
        "Video duration exceeds {} seconds",
        limits.max_duration_secs
    );
    let streams = report["streams"].as_array().context("No video streams")?;
    ensure!(
        !streams.is_empty() && streams.len() <= 2,
        "Only one video and at most one audio stream are allowed"
    );
    let mut video = None;
    let mut audio_count = 0;
    for stream in streams {
        if let Some(duration) = seconds(&stream["duration"]) {
            ensure!(
                duration > 0.0 && duration <= limits.max_duration_secs,
                "A stream exceeds the duration limit"
            );
        }
        match stream["codec_type"].as_str() {
            Some("video") => {
                ensure!(video.is_none(), "Only one video stream is allowed");
                ensure!(
                    stream["disposition"]["attached_pic"].as_u64().unwrap_or(0) == 0,
                    "Cover images are not videos"
                );
                let codec = stream["codec_name"]
                    .as_str()
                    .context("Unknown video codec")?;
                let allowed = match extension {
                    "webm" => matches!(codec, "vp8" | "vp9" | "av1"),
                    "mp4" => matches!(codec, "h264" | "hevc" | "av1" | "vp9" | "mpeg4"),
                    _ => false,
                };
                ensure!(allowed, "Unsupported video codec: {codec}");
                video = Some(dimensions(stream, limits)?);
            }
            Some("audio") => {
                audio_count += 1;
                ensure!(audio_count <= 1, "Only one audio stream is allowed");
                let codec = stream["codec_name"]
                    .as_str()
                    .context("Unknown audio codec")?;
                ensure!(
                    match extension {
                        "webm" => matches!(codec, "opus" | "vorbis"),
                        "mp4" => matches!(codec, "aac" | "mp3" | "opus" | "alac" | "ac3" | "eac3"),
                        _ => false,
                    },
                    "Unsupported audio codec: {codec}"
                );
            }
            _ => bail!("Subtitles, data streams and attachments are not allowed"),
        }
    }
    let (width, height) = video.context("The file contains no video")?;
    let frames = report["frames"]
        .as_array()
        .context("Video frames could not be decoded")?;
    let mut video_frames = 0;
    let mut first = f64::INFINITY;
    let mut last = f64::NEG_INFINITY;
    for frame in frames {
        if frame["media_type"] == "video" {
            dimensions(frame, limits)?;
            video_frames += 1;
        }
        let timestamp = seconds(&frame["best_effort_timestamp_time"])
            .context("Decoded frame is missing a valid timestamp")?;
        let frame_duration = seconds(&frame["duration_time"])
            .or_else(|| seconds(&frame["pkt_duration_time"]))
            .unwrap_or(0.0);
        ensure!(frame_duration >= 0.0, "Invalid frame duration");
        first = first.min(timestamp);
        last = last.max(timestamp + frame_duration);
    }
    ensure!(video_frames > 0, "No video frame could be decoded");
    // A small negative audio priming timestamp is normal. The actual media span must fit.
    ensure!(
        last.is_finite() && first.is_finite() && last - first <= limits.max_duration_secs,
        "Decoded video exceeds {} seconds",
        limits.max_duration_secs
    );
    Ok((duration.max(last - first), width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn report() -> serde_json::Value {
        json!({"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"1.0"},
            "streams":[{"codec_type":"video","codec_name":"h264","width":640,"height":480}],
            "frames":[{"media_type":"video","width":640,"height":480,"best_effort_timestamp_time":"0","duration_time":"1"}]})
    }
    #[test]
    fn header_and_size_fail_closed() {
        let mut bytes = vec![0; 16];
        bytes[4..8].copy_from_slice(b"ftyp");
        assert!(check_header(&bytes, "mp4", &Limits::default()).is_ok());
        assert!(check_header(&bytes, "webm", &Limits::default()).is_err());
        assert!(check_header(&bytes, "exe", &Limits::default()).is_err());
        assert!(
            check_header(
                &bytes,
                "mp4",
                &Limits {
                    max_bytes: 15,
                    ..Limits::default()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn limits_check_headers_and_decoded_frames() {
        assert!(check_report(&report(), "mp4", &Limits::default()).is_ok());
        let mut value = report();
        value["frames"][0]["width"] = json!(2161);
        assert!(check_report(&value, "mp4", &Limits::default()).is_err());
        let mut value = report();
        value["frames"][0]["duration_time"] = json!(181);
        assert!(check_report(&value, "mp4", &Limits::default()).is_err());
        let mut value = report();
        value["format"]["duration"] = json!("NaN");
        assert!(check_report(&value, "mp4", &Limits::default()).is_err());
    }
    #[test]
    fn rejects_hidden_streams_and_unknown_duration() {
        let mut value = report();
        value["streams"]
            .as_array_mut()
            .unwrap()
            .push(json!({"codec_type":"data"}));
        assert!(check_report(&value, "mp4", &Limits::default()).is_err());
        let mut value = report();
        value["format"].as_object_mut().unwrap().remove("duration");
        assert!(check_report(&value, "mp4", &Limits::default()).is_err());
    }
}

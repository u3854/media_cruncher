// src/compressors.rs

use std::process::{Command, Output};
use std::path::Path;
use crate::CompressionConfig;

pub fn compress_audio(input_path: &Path, output_path: &Path, config: &CompressionConfig) -> bool {
    let input_str: &str = input_path.to_str().expect("Invalid input path");
    let output_str: &str = output_path.to_str().expect("Invalid output path");
    let bitrate_str: String = format!("{}k", config.audio_bitrate_k);

    let mut cmd: Command = Command::new("ffmpeg");
    cmd.args([
        "-y",
        "-i", input_str,
        "-map_metadata", "-1",
        "-codec:a", "libopus",
        "-vn", 
        "-b:a", &bitrate_str,
        "-compression_level", "10",
        "-vbr", "on",
        "-f", "opus", // <-- EXPLICIT FORMAT FIX
        output_str,
    ]);

    handle_output(cmd.output())
}

pub fn compress_image(input_path: &Path, output_path: &Path, config: &CompressionConfig, ext: &str) -> bool {
    let input_str: &str = input_path.to_str().expect("Invalid input path");
    let output_str: &str = output_path.to_str().expect("Invalid output path");
    let quality_str: String = config.image_quality.to_string();

    let mut cmd: Command;

    if ext == "webp" {
        cmd = Command::new("ffmpeg");
        cmd.args([
            "-y", "-i", input_str, "-c:v", "libwebp", "-qscale:v", &quality_str, 
            "-f", "webp", // <-- EXPLICIT FORMAT FIX
            output_str
        ]);
    } else {
        cmd = Command::new("cwebp");
        cmd.args([
            input_str, "-q", &quality_str, "-m", "5", "-mt", "-o", output_str,
        ]);
    }

    handle_output(cmd.output())
}

pub fn compress_video(input_path: &Path, output_path: &Path, config: &CompressionConfig, ext: &str) -> bool {
    let input_str: &str = input_path.to_str().expect("Invalid input path");
    let output_str: &str = output_path.to_str().expect("Invalid output path");
    let q_factor_str: String = config.video_q_factor.to_string();
    let audio_bitrate_str: String = format!("{}k", config.audio_bitrate_k);

    let mut cmd: Command = Command::new("ffmpeg");

    if ext == "gif" {
        cmd.args([
            "-y", "-i", input_str, "-vcodec", "webp", "-pix_fmt", "yuv420p", 
            "-f", "webp", // <-- EXPLICIT FORMAT FIX
            output_str,
        ]);
    } else {
        // This one already had "-f webm" in your original script, so it was safe!
        cmd.args([
            "-y", "-i", input_str, "-hide_banner", "-c:v", "libvpx-vp9", "-b:v", "0",
            "-crf", &q_factor_str, "-g", "240", "-vsync", "2", "-row-mt", "1",
            "-frame-parallel", "1", "-auto-alt-ref", "1", "-lag-in-frames", "25",
            "-c:a", "libopus", "-vbr", "on", "-compression_level", "10",
            "-frame_duration", "60", "-application", "audio", "-b:a", &audio_bitrate_str,
            "-f", "webm", output_str,
        ]);
    }

    handle_output(cmd.output())
}

// --- NEW HELPER FUNCTION ---
// This takes the result of the command execution and gracefully handles errors
fn handle_output(result: Result<Output, std::io::Error>) -> bool {
    match result {
        Ok(output) => {
            if output.status.success() {
                true
            } else {
                // If FFmpeg fails, extract its error text from stderr and print it
                let err_msg: String = String::from_utf8_lossy(&output.stderr).to_string();
                println!("  [ENCODER ERROR]:\n{}", err_msg.trim());
                false
            }
        }
        Err(e) => {
            // This catches OS errors (like if the system forcefully killed the process)
            println!("  [SYSTEM ERROR]: Command failed to execute - {}", e);
            false
        }
    }
}
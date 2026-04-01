use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::process::{Command, Output};
use std::path::Path;
use crate::CompressionConfig;

pub fn compress_audio(input_path: &Path, output_path: &Path, config: &CompressionConfig, ext: &str) -> bool {
    let input_str = input_path.to_str().expect("Invalid input path");
    let output_str = output_path.to_str().expect("Invalid output path");
    let bitrate_str = format!("{}k", config.audio_bitrate_k);

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-i", input_str, "-map_metadata", "-1", "-vn"]);

    match ext {
        "wav" | "opus" => {
            // .wav gets the Opus trenchcoat. .opus stays Opus.
            cmd.args(["-c:a", "libopus", "-b:a", &bitrate_str, "-vbr", "on", "-compression_level", "10", "-f", "opus"]);
        }
        "ogg" => {
            cmd.args(["-c:a", "libopus", "-b:a", &bitrate_str, "-vbr", "on", "-f", "ogg"]);
        }
        "mp3" => {
            cmd.args(["-c:a", "libmp3lame", "-b:a", &bitrate_str, "-f", "mp3"]);
        }
        _ => return false, // Unsupported audio
    }
    
    cmd.arg(output_str);
    handle_output(cmd.output())
}

pub fn compress_image(input_path: &Path, output_path: &Path, config: &CompressionConfig, ext: &str) -> bool {
    let input_str = input_path.to_str().expect("Invalid input path");
    let output_str = output_path.to_str().expect("Invalid output path");

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-i", input_str]);

    // Apply scaling if a resolution target is set
    if let Some(max_res) = config.max_resolution_px {
        let scale_filter = format!("scale={}:{}:force_original_aspect_ratio=decrease", max_res, max_res);
        cmd.args(["-vf", &scale_filter]);
    }

    let mut is_webp_output = false;

    match ext {
        "png" | "webp" => {
            // .png gets the WebP trenchcoat. .webp stays WebP.
            cmd.args(["-c:v", "libwebp", "-qscale:v", &config.img_webp_q.to_string(), "-f", "webp"]);
            is_webp_output = true;
        }
        "jpg" | "jpeg" => {
            cmd.args(["-c:v", "mjpeg", "-q:v", &config.img_jpg_q.to_string(), "-f", "image2"]);
        }
        _ => return false,
    }

    cmd.arg(output_str);
    let success = handle_output(cmd.output());

    // Natively patch the RIFF header if we generated WebP data
    if success && is_webp_output {
        if let Err(e) = fix_webp_header(output_path) {
            println!("  [WARNING] Failed to patch WebP header on {}: {}", output_path.display(), e);
        }
    }

    success
}

pub fn compress_video(input_path: &Path, output_path: &Path, config: &CompressionConfig, ext: &str) -> bool {
    let input_str = input_path.to_str().expect("Invalid input path");
    let output_str = output_path.to_str().expect("Invalid output path");
    let bitrate_str = format!("{}k", config.audio_bitrate_k);

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-i", input_str, "-hide_banner"]);

    // Apply scaling and ensure divisible-by-2 dimensions for video encoders
    if let Some(max_res) = config.max_resolution_px {
        let scale_filter = format!(
            "scale={}:{}:force_original_aspect_ratio=decrease,pad=ceil(iw/2)*2:ceil(ih/2)*2", 
            max_res, max_res
        );
        cmd.args(["-vf", &scale_filter]);
    }

    match ext {
        "mp4" | "mkv" | "avi" => {
            cmd.args([
                "-c:v", "libx264", 
                "-crf", &config.video_x264_crf.to_string(), 
                "-preset", "fast", // Faster encoding, good compression
                "-c:a", "aac", // Universal audio codec for these containers
                "-b:a", &bitrate_str
            ]);
            if ext == "mp4" { cmd.args(["-f", "mp4"]); }
            else if ext == "mkv" { cmd.args(["-f", "matroska"]); }
            else { cmd.args(["-f", "avi"]); }
        }
        "webm" | "ogv" => {
            cmd.args([
                "-c:v", "libvpx-vp9", "-b:v", "0", 
                "-crf", &config.video_vp9_crf.to_string(), 
                "-row-mt", "1", "-c:a", "libopus", "-b:a", &bitrate_str, 
                "-f", "webm"
            ]);
        }
        "gif" => {
            cmd.args(["-vcodec", "webp", "-pix_fmt", "yuv420p", "-f", "webp"]);
        }
        _ => return false,
    }

    cmd.arg(output_str);
    handle_output(cmd.output())
}

// --- NATIVE WEBP HEADER FIX ---
// This replaces the Python script. It directly modifies the binary file.
fn fix_webp_header(path: &Path) -> std::io::Result<()> {
    let size = fs::metadata(path)?.len();
    if size < 12 { return Ok(()); } // File is too small to be a valid WebP

    // Open file in Write mode (without truncating it)
    let mut file = OpenOptions::new().write(true).open(path)?;
    
    // Calculate size minus 8 bytes (RIFF specification)
    let riff_size = (size - 8) as u32;
    
    // Jump exactly to the 4th byte
    file.seek(SeekFrom::Start(4))?;
    
    // Overwrite the next 4 bytes with our corrected little-endian integer
    file.write_all(&riff_size.to_le_bytes())?;
    
    Ok(())
}

fn handle_output(result: Result<Output, std::io::Error>) -> bool {
    match result {
        Ok(output) => {
            if output.status.success() { true } else {
                let err_msg = String::from_utf8_lossy(&output.stderr).to_string();
                println!("  [ENCODER ERROR]:\n{}", err_msg.trim());
                false
            }
        }
        Err(e) => {
            println!("  [SYSTEM ERROR]: Command failed to execute - {}", e);
            false
        }
    }
}
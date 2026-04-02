mod compressors;
use compressors::{compress_audio, compress_image, compress_video};

mod archive;
// derive_zip_output_path is removed since we replace in place
use archive::{extract_archive, is_supported_archive, repack_zip};

use tempfile::TempDir;
use walkdir::{WalkDir, DirEntry};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "A simple media cruncher")]
struct Args {
    #[arg(short = 'p', long = "path", default_value = ".")]
    path: String,

    /// 'mobile' or 'full-hd'
    #[arg(short = 'm', long = "mode", default_value = "full-hd")]
    mode: String,

    /// Optional target max resolution in pixels, e.g. 1920 or 1280
    #[arg(short = 'r', long = "resolution")]
    resolution: Option<u32>,

    /// Number of threads: 'max' or a specific number
    #[arg(short = 't', long = "threads")]
    threads: Option<String>,
}

#[derive(Debug)]
enum MediaType {
    Image,
    Video,
    Audio,
    Ignore,
}

#[derive(Debug)]
pub struct CompressionConfig {
    pub img_webp_q: u8,
    pub img_jpg_q: u8,
    pub audio_bitrate_k: u8,
    pub video_x264_crf: u8,
    pub video_vp9_crf: u8,
    pub max_resolution_px: Option<u32>,
}

impl CompressionConfig {
    pub fn new(level: &str, max_resolution_px: Option<u32>) -> CompressionConfig {

        match level.to_lowercase().as_str() {
            "mobile" => CompressionConfig {
                img_webp_q: 60,
                img_jpg_q: 7,
                audio_bitrate_k: 64,
                video_x264_crf: 28,
                video_vp9_crf: 40,
                max_resolution_px,
            },
            _ => CompressionConfig {
                img_webp_q: 80,
                img_jpg_q: 2,
                audio_bitrate_k: 128,
                video_x264_crf: 23,
                video_vp9_crf: 31,
                max_resolution_px,
            },
        }
    }
}

fn determine_media_type(ext: &str) -> MediaType {
    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" => MediaType::Image,
        "webm" | "mp4" | "mkv" | "ogv" | "avi" | "mpg" | "m4v" | "gif" => MediaType::Video,
        "mp3" | "wav" | "ogg" | "opus" => MediaType::Audio,
        _ => MediaType::Ignore,
    }
}

fn check_dependencies() -> bool {
    let required_tools: [(&str, &[&str]); 2] = [
        ("ffmpeg", &["-version"]),
        ("7z", &["-h"]),
    ];

    let mut all_tools_found = true;

    println!("Checking system dependencies...");

    for (tool, args) in required_tools {
        let result: Result<std::process::Output, std::io::Error> =
            Command::new(tool).args(args).output();

        match result {
            Ok(_) => println!("  [OK] Found {}", tool),
            Err(_) => {
                println!("  [ERROR] Missing required tool: {}", tool);
                all_tools_found = false;
            }
        }
    }

    all_tools_found
}

fn format_bytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb > 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{:.2} MB", mb)
    }
}

fn main() {
    let args: Args = Args::parse();

    let max_cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

    let target_threads = match args.threads.as_deref() {
        Some("max") => max_cores,
        Some(num_str) => num_str.parse::<usize>().unwrap_or(std::cmp::max(1, max_cores / 2)),
        None => std::cmp::max(1, max_cores / 2),
    };

    let final_threads = target_threads.clamp(1, max_cores);
    rayon::ThreadPoolBuilder::new().num_threads(final_threads).build_global().unwrap();

    let deps_ok = check_dependencies();
    if !deps_ok {
        println!("Please install missing dependencies and ensure they are in your PATH.");
        std::process::exit(1);
    }

    let config: CompressionConfig = CompressionConfig::new(&args.mode, args.resolution);

    println!("🚀 Engine initialized with {}/{} CPU threads.", final_threads, max_cores);
    println!("Target Path: {}", args.path);
    println!("Using config: {:?}\n", config);

    let input_path: &Path = Path::new(&args.path);
    let is_archive: bool = input_path.is_file() && is_supported_archive(input_path);

    let _temp_dir_holder: Option<TempDir>;
    let mut target_dir_path: PathBuf = PathBuf::from(&args.path);

    if is_archive {
        println!("Identified archive file: {}", input_path.display());

        let temp_dir: TempDir = tempfile::Builder::new()
            .prefix("media_cruncher_")
            .tempdir()
            .expect("Failed to create temporary directory");

        extract_archive(input_path, temp_dir.path()).expect("Failed to extract archive");
        target_dir_path = temp_dir.path().to_path_buf();
        _temp_dir_holder = Some(temp_dir);
    } else {
        _temp_dir_holder = None;
    }

    let target_dir: &str = target_dir_path.to_str().expect("Invalid path");

    let mut image_files: Vec<PathBuf> = Vec::new();
    let mut video_files: Vec<PathBuf> = Vec::new();
    let mut audio_files: Vec<PathBuf> = Vec::new();

    for entry_result in WalkDir::new(target_dir) {
        let entry: DirEntry = entry_result.expect("Failed to read a file or folder");
        let path: &Path = entry.path();

        if path.is_file() {
            if let Some(ext_osstr) = path.extension() {
                if let Some(ext_str) = ext_osstr.to_str() {
                    let ext_lower: String = ext_str.to_lowercase();
                    match determine_media_type(&ext_lower) {
                        MediaType::Image => image_files.push(path.to_path_buf()),
                        MediaType::Video => video_files.push(path.to_path_buf()),
                        MediaType::Audio => audio_files.push(path.to_path_buf()),
                        MediaType::Ignore => {}
                    }
                }
            }
        }
    }

    println!("Scan Complete:");
    println!("  Images found: {}", image_files.len());
    println!("  Videos found: {}", video_files.len());
    println!("  Audio files found: {}", audio_files.len());

    let total_original_bytes = AtomicU64::new(0);
    let total_final_bytes = AtomicU64::new(0);

    let pb_style = ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
        .expect("Failed to create progress style")
        .progress_chars("#>-");

    // --- Image Processing ---
    if !image_files.is_empty() {
        println!("\n📸 Compressing {} Images...", image_files.len());
        let pb = ProgressBar::new(image_files.len() as u64);
        pb.set_style(pb_style.clone());

        image_files.into_par_iter().for_each(|path| {
            let ext_osstr = path.extension().expect("File has no extension");
            let ext_str = ext_osstr.to_str().expect("Invalid extension text").to_lowercase();
            let temp_path: PathBuf = path.with_extension("tmp");

            let orig_size: u64 = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            total_original_bytes.fetch_add(orig_size, Ordering::Relaxed);

            let success: bool = compress_image(&path, &temp_path, &config, &ext_str);

            if success {
                let new_size: u64 = fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0);
                if new_size > 0 && new_size < orig_size {
                    let _ = fs::remove_file(&path);
                    let _ = fs::rename(&temp_path, &path);
                    total_final_bytes.fetch_add(new_size, Ordering::Relaxed);
                } else {
                    let _ = fs::remove_file(&temp_path);
                    total_final_bytes.fetch_add(orig_size, Ordering::Relaxed);
                }
            } else {
                total_final_bytes.fetch_add(orig_size, Ordering::Relaxed);
                pb.println(format!("  [FAILED] {}", path.display()));
                if temp_path.exists() {
                    let _ = fs::remove_file(&temp_path);
                }
            }

            pb.inc(1);
        });

        pb.finish_with_message("Done!");
    }

    // --- Audio Processing ---
    if !audio_files.is_empty() {
        println!("\n🎵 Compressing {} Audio Files...", audio_files.len());
        let pb = ProgressBar::new(audio_files.len() as u64);
        pb.set_style(pb_style.clone());

        audio_files.into_par_iter().for_each(|path| {
            let ext_osstr = path.extension().expect("File has no extension");
            let ext_str = ext_osstr.to_str().expect("Invalid extension text").to_lowercase();
            let temp_path: PathBuf = path.with_extension("tmp");
            let orig_size: u64 = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            total_original_bytes.fetch_add(orig_size, Ordering::Relaxed);

            let success: bool = compress_audio(&path, &temp_path, &config, &ext_str);

            if success {
                let new_size: u64 = fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0);
                if new_size > 0 && new_size < orig_size {
                    let _ = fs::remove_file(&path);
                    let _ = fs::rename(&temp_path, &path);
                    total_final_bytes.fetch_add(new_size, Ordering::Relaxed);
                } else {
                    let _ = fs::remove_file(&temp_path);
                    total_final_bytes.fetch_add(orig_size, Ordering::Relaxed);
                }
            } else {
                total_final_bytes.fetch_add(orig_size, Ordering::Relaxed);
                pb.println(format!("  [FAILED] {}", path.display()));
                if temp_path.exists() {
                    let _ = fs::remove_file(&temp_path);
                }
            }

            pb.inc(1);
        });

        pb.finish_with_message("Done!");
    }

    // --- Video Processing ---
    if !video_files.is_empty() {
        println!("\n🎬 Compressing {} Videos...", video_files.len());
        let pb = ProgressBar::new(video_files.len() as u64);
        pb.set_style(pb_style.clone());

        video_files.into_par_iter().for_each(|path| {
            let ext_osstr = path.extension().expect("File has no extension");
            let ext_str = ext_osstr.to_str().expect("Invalid extension text").to_lowercase();
            let temp_path: PathBuf = path.with_extension("tmp");
            let orig_size: u64 = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            total_original_bytes.fetch_add(orig_size, Ordering::Relaxed);

            let success: bool = compress_video(&path, &temp_path, &config, &ext_str);

            if success {
                let new_size: u64 = fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0);
                if new_size > 0 && new_size < orig_size {
                    let _ = fs::remove_file(&path);
                    let _ = fs::rename(&temp_path, &path);
                    total_final_bytes.fetch_add(new_size, Ordering::Relaxed);
                } else {
                    let _ = fs::remove_file(&temp_path);
                    total_final_bytes.fetch_add(orig_size, Ordering::Relaxed);
                }
            } else {
                total_final_bytes.fetch_add(orig_size, Ordering::Relaxed);
                pb.println(format!("  [FAILED] {}", path.display()));
                if temp_path.exists() {
                    let _ = fs::remove_file(&temp_path);
                }
            }

            pb.inc(1);
        });

        pb.finish_with_message("Done!");
    }

    println!("\n🎉 All processing complete!");

    let orig_total = total_original_bytes.load(Ordering::Relaxed);
    let final_total = total_final_bytes.load(Ordering::Relaxed);

    if orig_total > 0 {
        let saved_bytes = orig_total.saturating_sub(final_total);
        let reduction_percent = (saved_bytes as f64 / orig_total as f64) * 100.0;

        println!("📊 Size Summary:");
        println!("  Original Size : {}", format_bytes(orig_total));
        println!("  Final Size    : {}", format_bytes(final_total));
        println!("  Space Saved   : {} ({:.1}% reduction)", format_bytes(saved_bytes), reduction_percent);
    }

    // --- The In-Place Archive Replacement Logic ---
    if is_archive {
        // Determine the final extension. If it's cbz or zip, keep it. 
        // If it was rar or 7z, we force it to zip so the OS doesn't get confused by the format.
        let orig_ext = input_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let final_ext = match orig_ext.as_str() {
            "cbz" => "cbz",
            "zip" => "zip",
            _ => "zip", 
        };
        
        let final_archive_path = input_path.with_extension(final_ext);
        
        // Create a temporary archive file in the same directory as the original
        let temp_archive_path = input_path.with_extension("tmp.archive");

        repack_zip(&target_dir_path, &temp_archive_path)
            .expect("Failed to repack archive as ZIP");
            
        // If the original file is different from the target file (e.g., we are changing .rar to .zip),
        // delete the original first to avoid leaving duplicate data.
        if input_path != final_archive_path {
            let _ = fs::remove_file(input_path);
        }

        // Atomically rename the temporary repacked archive over the target path
        fs::rename(&temp_archive_path, &final_archive_path)
            .expect("Failed to replace the original archive");

        println!("✅ Successfully updated archive in place: {}", final_archive_path.display());
    }
}
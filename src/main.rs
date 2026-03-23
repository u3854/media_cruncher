mod compressors;
use compressors::{compress_audio, compress_image, compress_video};

mod archive; // Tell Rust our new module exists
use archive::{extract_zip, repack_zip};

use tempfile::TempDir;
use walkdir::{WalkDir, DirEntry};
use std::path::{Path, PathBuf};
use std::ffi::OsStr;
use std::process::Command;
use std::fs;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

// --- NEW CLAP IMPORT ---
use clap::Parser;

// --- DEFINING OUR CLI ARGUMENTS ---

// The `///` comments here are special. `clap` actually reads them 
// and turns them into the --help menu text in your terminal!
#[derive(Parser, Debug)]
#[command(author, version, about = "A simple media cruncher")]
struct Args {
    #[arg(short = 'p', long = "path", default_value = ".")]
    path: String,

    #[arg(short = 'm', long = "mode", default_value = "full-hd")]
    mode: String,

    /// Optional target resolution: '1080p' or '720p'
    #[arg(short = 'r', long = "resolution")]
    resolution: Option<String>,

    /// Number of threads: 'max' or a specific number
    #[arg(short = 't', long = "threads")]
    threads: Option<String>,
}

// --- DEFINING OUR TYPES ---

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
    pub max_resolution_px: Option<u32>, // Holds the longest-edge limit
}

impl CompressionConfig {
    pub fn new(level: &str, res_arg: Option<&String>) -> CompressionConfig {
        // Parse the resolution string into a pixel limit for the longest side
        let max_resolution_px = match res_arg.map(|s| s.as_str()) {
            Some("1080p") | Some("1080") => Some(1920),
            Some("720p") | Some("720") => Some(1280),
            _ => None,
        };

        match level.to_lowercase().as_str() {
            "mobile" => CompressionConfig {
                img_webp_q: 60, img_jpg_q: 7, audio_bitrate_k: 64,
                video_x264_crf: 28, video_vp9_crf: 40,
                max_resolution_px,
            },
            _ => CompressionConfig { 
                img_webp_q: 80, img_jpg_q: 2, audio_bitrate_k: 128,
                video_x264_crf: 23, video_vp9_crf: 31,
                max_resolution_px,
            },
        }
    }
}

// --- HELPER FUNCTIONS ---

// Categorize files based on their extension
fn determine_media_type(ext: &str) -> MediaType {
    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" => MediaType::Image,
        "webm" | "mp4" | "mkv" | "ogv" | "avi" | "mpg" | "m4v" | "gif" => MediaType::Video,
        "mp3" | "wav" | "ogg" | "opus" => MediaType::Audio,
        _ => MediaType::Ignore, 
    }
}

// Check if ffmpeg is installed and accessible
fn check_dependencies() -> bool {
    // We removed cwebp and nconvert because ffmpeg handles everything now!
    let required_tools: [&str; 1] = ["ffmpeg"];
    let mut all_tools_found: bool = true;

    println!("Checking system dependencies...");

    for tool in required_tools {
        // Spawning the command with "-version" to see if the OS finds it
        let result: Result<std::process::Output, std::io::Error> = Command::new(tool).arg("-version").output();

        match result {
            Ok(_) => {
                println!("  [OK] Found {}", tool);
            }
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

// --- MAIN EXECUTION ---

fn main() -> () {

    let args: Args = Args::parse();

    // --- THREAD POOL CONFIGURATION ---
    // Ask the OS for total logical cores
    let max_cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    
    // Calculate how many threads to use based on user input
    let target_threads = match args.threads.as_deref() {
        Some("max") => max_cores,
        Some(num_str) => num_str.parse::<usize>().unwrap_or(std::cmp::max(1, max_cores / 2)),
        None => std::cmp::max(1, max_cores / 2), // Default to 50% capacity
    };
    
    // Mathematically clamp it between 1 and the absolute hardware max
    let final_threads = target_threads.clamp(1, max_cores);
    
    // Lock the global thread pool
    rayon::ThreadPoolBuilder::new().num_threads(final_threads).build_global().unwrap();
    
    // Check dependencies before doing anything else
    let deps_ok: bool = check_dependencies();
    if !deps_ok {
        println!("Please install missing dependencies and ensure they are in your PATH.");
        // Exit with an error code if tools are missing
        std::process::exit(1); 
    }

    // --- CONFIGURATION INIT ---
    // Pass the optional resolution argument into our config builder
    let config: CompressionConfig = CompressionConfig::new(&args.mode, args.resolution.as_ref());
    
    println!("🚀 Engine initialized with {}/{} CPU threads.", final_threads, max_cores);
    println!("Target Path: {}", args.path);
    println!("Using config: {:?}\n", config);

    // --- NEW ZIP INTERCEPTION LOGIC ---
    let input_path: &Path = Path::new(&args.path);
    let is_zip: bool = input_path.is_file() && 
        (input_path.extension() == Some(OsStr::new("zip")) || 
        input_path.extension() == Some(OsStr::new("cbz")));

    // We declare a variable to hold our TempDir. It's an Option because we 
    // only create it if the input is actually a zip file.
    let _temp_dir_holder: Option<TempDir>; 
    let mut target_dir_path: PathBuf = PathBuf::from(&args.path);

    if is_zip {
        println!("Identified archive file: {}", input_path.display());
        
        // Create the temporary directory securely
        let temp_dir: TempDir = tempfile::Builder::new()
            .prefix("media_cruncher_")
            .tempdir()
            .expect("Failed to create temporary directory");
            
        // Extract the zip into the temp directory
        extract_zip(input_path, temp_dir.path()).expect("Failed to extract zip file");
        
        // Change our target directory so WalkDir processes the temp folder instead!
        target_dir_path = temp_dir.path().to_path_buf();
        
        // Keep the TempDir alive by moving it into our holder variable
        _temp_dir_holder = Some(temp_dir);
    } else {
        _temp_dir_holder = None;
    }

    // Now, instead of &args.path, WalkDir will use our target_dir_path 
    // (which is either the original folder, or our new temporary extracted folder).
    let target_dir: &str = target_dir_path.to_str().expect("Invalid path"); 

    // Prepare our Vectors (Lists) to hold the found files
    let mut image_files: Vec<PathBuf> = Vec::new();
    let mut video_files: Vec<PathBuf> = Vec::new();
    let mut audio_files: Vec<PathBuf> = Vec::new();

    // Walk the directory
    for entry_result in WalkDir::new(target_dir) {
        let entry: DirEntry = entry_result.expect("Failed to read a file or folder");
        let path: &Path = entry.path();
        
        if path.is_file() {
            let ext_osstr_option: Option<&OsStr> = path.extension();

            if let Some(ext_osstr) = ext_osstr_option {
                let ext_str_option: Option<&str> = ext_osstr.to_str();

                if let Some(ext_str) = ext_str_option {
                    let ext_lower: String = ext_str.to_lowercase();
                    let media_type: MediaType = determine_media_type(&ext_lower);

                    // Route the file to the correct Vector
                    match media_type {
                        MediaType::Image => image_files.push(path.to_path_buf()),
                        MediaType::Video => video_files.push(path.to_path_buf()),
                        MediaType::Audio => audio_files.push(path.to_path_buf()),
                        MediaType::Ignore => {} 
                    }
                }
            }
        }
    }

    // Print the summary
    println!("Scan Complete:");
    println!("  Images found: {}", image_files.len());
    println!("  Videos found: {}", video_files.len());
    println!("  Audio files found: {}", audio_files.len());

    // --- EXECUTION PHASE ---
    
    // We use AtomicU64 so multiple threads can safely add to the totals at the same time
    let total_original_bytes = AtomicU64::new(0);
    let total_final_bytes = AtomicU64::new(0);

    let pb_style = ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
        .expect("Failed to create progress style")
        .progress_chars("#>-");

    // 1. PROCESS IMAGES (PARALLEL)
    if !image_files.is_empty() {
        println!("\n📸 Compressing {} Images...", image_files.len());
        let pb = ProgressBar::new(image_files.len() as u64);
        pb.set_style(pb_style.clone());

        // .into_par_iter() replaces the standard 'for' loop.
        // It consumes the vector and hands the items out to available CPU cores.
        image_files.into_par_iter().for_each(|path| {
            let ext_osstr = path.extension().expect("File has no extension");
            let ext_str = ext_osstr.to_str().expect("Invalid extension text").to_lowercase();
            let temp_path: PathBuf = path.with_extension("tmp");
            
            let orig_size: u64 = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            
            // fetch_add safely adds the number across multiple threads
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
                if temp_path.exists() { let _ = fs::remove_file(&temp_path); }
            }
            
            // indicatif progress bars are magically thread-safe by default!
            pb.inc(1); 
        });
        
        pb.finish_with_message("Done!");
    }

    // 2. PROCESS AUDIO (PARALLEL)
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
                if temp_path.exists() { let _ = fs::remove_file(&temp_path); }
            }
            pb.inc(1);
        });
        pb.finish_with_message("Done!");
    }

    // 3. PROCESS VIDEOS (PARALLEL)
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
                if temp_path.exists() { let _ = fs::remove_file(&temp_path); }
            }
            pb.inc(1);
        });
        pb.finish_with_message("Done!");
    }

    // --- FINAL SUMMARY ---
    println!("\n🎉 All processing complete!");
    
    // We load the final numbers out of the Atomics to do our math
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
    // --- REPACK ZIP IF NECESSARY ---
    if is_zip {
        // If it was a zip file, target_dir_path is pointing to our temp folder.
        // We pack it back into the original input_path.
        repack_zip(&target_dir_path, input_path).expect("Failed to repack zip file");
        println!("✅ Successfully repacked archive!");
    }
    
    // Right here, when the main function ends, _temp_dir_holder goes out of scope.
    // Rust automatically deletes the temporary directory and all its contents!
}
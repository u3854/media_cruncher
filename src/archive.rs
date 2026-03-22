use std::fs;
use std::fs::File;
use std::io;
use std::path::Path;
use zip::read::ZipArchive;
use zip::write::{FileOptions, ZipWriter};
// We bring in WalkDir to iterate over the temp folder when repacking
use walkdir::WalkDir;

// Extracts a zip file into a target directory.
// We return a Result so we can easily handle errors if the zip is corrupted.
pub fn extract_zip(zip_path: &Path, extract_to: &Path) -> Result<(), io::Error> {
    println!("📦 Extracting archive to temporary workspace...");
    
    // Open the zip file for reading
    let file: File = File::open(zip_path)?;
    let mut archive: ZipArchive<File> = ZipArchive::new(file)?;

    for i in 0..archive.len() {
        // We have to use `by_index` to get a mutable reference to each file inside the zip
        let mut zip_file = archive.by_index(i)?;
        
        // Determine where this file should go in our temporary directory
        // .enclosed_name() prevents "Zip Slip" security vulnerabilities (e.g., paths with "../")
        let out_path_option = zip_file.enclosed_name();
        
        if let Some(out_path) = out_path_option {
            let full_out_path = extract_to.join(out_path);

            if zip_file.is_dir() {
                // If the item in the zip is a folder, create that folder in our temp dir
                fs::create_dir_all(&full_out_path)?;
            } else {
                // If it's a file, ensure its parent folder exists, then copy the data
                if let Some(parent) = full_out_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut out_file: File = File::create(&full_out_path)?;
                // io::copy reads the bytes from the zip and writes them directly to the new file
                io::copy(&mut zip_file, &mut out_file)?;
            }
        }
    }
    Ok(())
}

// Zips a directory back up into a single file.
pub fn repack_zip(source_dir: &Path, zip_path: &Path) -> Result<(), io::Error> {
    println!("\n📦 Repacking archive...");
    
    // Open the zip file for writing (this will overwrite the original zip!)
    let file: File = File::create(zip_path)?;
    let mut zip_writer: ZipWriter<File> = ZipWriter::new(file);
    
    // We use the standard Deflate compression method (standard ZIP)
    let options: FileOptions<()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Walk through our processed temporary directory
    for entry_result in WalkDir::new(source_dir) {
        let entry = entry_result?;
        let path = entry.path();
        
        // We need to figure out the file's path *relative* to the temp folder 
        // so we don't accidentally zip the entire absolute Linux path (e.g., /tmp/mc_123/images/bg.png -> images/bg.png)
        let relative_path = path.strip_prefix(source_dir).unwrap_or(path);
        let relative_path_str: &str = relative_path.to_str().unwrap_or("");

        if path.is_file() {
            // Tell the zip writer we are starting a new file
            zip_writer.start_file(relative_path_str, options)?;
            
            // Read the processed file from disk and copy it into the zip
            let mut f: File = File::open(path)?;
            io::copy(&mut f, &mut zip_writer)?;
        } else if !relative_path_str.is_empty() {
            // It's a directory, so we just add a directory marker to the zip
            zip_writer.add_directory(relative_path_str, options)?;
        }
    }

    // Finalize the zip file to ensure all bytes are written safely
    zip_writer.finish()?;
    Ok(())
}
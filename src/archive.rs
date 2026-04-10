use std::fs;
use std::io::{self, Write};
use std::path::{Path};
use std::process::Command;

const SEVEN_ZIP: &str = "7z";

pub fn is_supported_archive(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    let name = name.to_lowercase();

    matches!(
        name.as_str(),
        s if s.ends_with(".zip")
            || s.ends_with(".cbz")
            || s.ends_with(".7z")
            || s.ends_with(".rar")
            || s.ends_with(".tar")
            || s.ends_with(".gz")
            || s.ends_with(".bz2")
            || s.ends_with(".xz")
            || s.ends_with(".tgz")
            || s.ends_with(".tbz2")
            || s.ends_with(".txz")
    ) || name.ends_with(".tar.gz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tar.xz")
}

fn archive_failure(action: &str, stderr: &str) -> io::Error {
    let msg = if stderr.trim().is_empty() {
        format!("7z failed to {action}")
    } else {
        format!("7z failed to {action}:\n{}", stderr.trim())
    };
    io::Error::new(io::ErrorKind::Other, msg)
}

fn looks_password_protected(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("wrong password")
        || s.contains("can not open encrypted archive")
        || s.contains("encrypted archive")
        || s.contains("data error in encrypted file")
        || s.contains("break signaled")
        || s.contains("headers error")
}

fn prompt_for_password() -> io::Result<String> {
    print!("Archive is password-protected. Enter password: ");
    io::stdout().flush()?;

    let mut password = String::new();
    io::stdin().read_line(&mut password)?;
    Ok(password.trim_end_matches(&['\r', '\n'][..]).to_string())
}

fn run_7z_extract(archive_path: &Path, extract_to: &Path, password: Option<&str>) -> io::Result<(bool, String)> {
    let mut cmd = Command::new(SEVEN_ZIP);

    cmd.arg("x")
        .arg("-y")
        .arg(format!("-o{}", extract_to.display()));

    if let Some(pw) = password {
        if !pw.is_empty() {
            cmd.arg(format!("-p{}", pw));
        } else {
            cmd.arg("-p");
        }
    }

    cmd.arg(archive_path);

    let output = cmd.output()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status.success(), stderr))
}

// Extract any 7z-supported archive into a directory.
// If the archive is encrypted, prompt the user for a password and retry.
pub fn extract_archive(archive_path: &Path, extract_to: &Path) -> Result<(), io::Error> {
    println!("📦 Extracting archive to temporary workspace...");

    let (ok, stderr) = run_7z_extract(archive_path, extract_to, None)?;
    if ok {
        return Ok(());
    }

    if looks_password_protected(&stderr) {
        for _ in 0..3 {
            let password = prompt_for_password()?;
            let (ok_retry, retry_stderr) =
                run_7z_extract(archive_path, extract_to, Some(password.as_str()))?;

            if ok_retry {
                return Ok(());
            }

            if !looks_password_protected(&retry_stderr) {
                return Err(archive_failure("extract archive", &retry_stderr));
            }

            println!("Wrong password. Try again.");
        }

        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Too many wrong password attempts.",
        ));
    }

    Err(archive_failure("extract archive", &stderr))
}

pub fn repack_zip(source_dir: &Path, zip_path: &Path) -> Result<(), io::Error> {
    println!("\n📦 Repacking archive as ZIP...");

    // 1. Force the path to be absolute so 7z doesn't save it in the temp folder
    let absolute_zip_path = if zip_path.is_absolute() {
        zip_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(zip_path)
    };

    if absolute_zip_path.exists() {
        fs::remove_file(&absolute_zip_path)?;
    }

    let mut cmd = Command::new("7z");
    
    // 2. Use "*" and let 7z handle the files to avoid OS command line length limits
    cmd.arg("a")
        .arg("-tzip")
        .arg("-mx=9")
        .arg(&absolute_zip_path)
        .arg("*") 
        .current_dir(source_dir);

    let output = cmd.output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error_msg = if stderr.trim().is_empty() {
            "7z failed to create the ZIP archive with no error output.".to_string()
        } else {
            format!("7z failed to create the ZIP archive:\n{}", stderr.trim())
        };
        
        Err(io::Error::new(io::ErrorKind::Other, error_msg))
    }
}
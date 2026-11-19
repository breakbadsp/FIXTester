mod ui;

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use eframe::egui;

use fixtester_desktop::{Result, invalid};

fn main() -> std::process::ExitCode {
    if let Err(error) = run() {
        eprintln!("FIXTester: {error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

fn arguments() -> Result<(PathBuf, bool)> {
    let mut args = std::env::args_os().skip(1);
    let mut directory = None;
    let mut smoke = false;
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            directory = Some(PathBuf::from(
                args.next()
                    .ok_or_else(|| invalid("--data-dir requires a path."))?,
            ));
        } else if arg == "--smoke" {
            smoke = true;
        } else {
            return Err(invalid("Usage: fixtester [--data-dir PATH] [--smoke]"));
        }
    }
    if smoke && directory.is_none() {
        return Err(invalid("--smoke requires a separate --data-dir."));
    }
    let default_directory = || {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .map(|base| base.join("fixtester"))
    };
    Ok((
        directory
            .or_else(default_directory)
            .ok_or_else(|| invalid("Set --data-dir, XDG_DATA_HOME, or HOME."))?,
        smoke,
    ))
}

fn lock_database(path: &Path) -> Result<(PathBuf, File)> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let database = match std::fs::canonicalize(path) {
        Ok(database) => database,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path)?;
            std::fs::canonicalize(path)?
        }
        Err(error) => return Err(error.into()),
    };
    if std::fs::metadata(&database)?.nlink() != 1 {
        return Err(invalid("The database must not have hard links."));
    }
    let mut lock_path = database.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)?;
    lock.try_lock()
        .map_err(|error| invalid(format!("Cannot lock the database: {error}")))?;
    Ok((database, lock))
}

fn run() -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let (directory, smoke) = arguments()?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)?;
    let (database, _lock) = lock_database(&directory.join("fixtester.sqlite3"))?;
    let rendered = Arc::new(AtomicBool::new(false));
    let report = smoke.then(|| Arc::clone(&rendered));
    let app = ui::Workspace::open(&database, report)?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([860.0, 580.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "FIXTester",
        options,
        Box::new(move |context| {
            ui::style(&context.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| invalid(error.to_string()))?;
    if smoke {
        if !rendered.load(Ordering::Relaxed) {
            return Err(invalid("The FIXTester frame was not verified."));
        }
        println!("Verified a rendered FIXTester workspace.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn aliases_share_the_database_lock_and_preserve_path_bytes() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let name = std::ffi::OsStr::from_bytes(b"fix-\xff.sqlite3");
        let path = directory.path().join(name);
        let alias = directory.path().join("alias.sqlite3");
        let (database, lock) = lock_database(&path)?;
        let store = fixtester_desktop::store::Store::open(&database)?;
        std::os::unix::fs::symlink(&database, &alias)?;
        assert!(lock_database(&alias).is_err());
        drop(store);
        drop(lock);
        assert_eq!(lock_database(&alias)?.0, database);
        Ok(())
    }
}

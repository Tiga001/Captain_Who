pub(crate) use mycopilot_core::storage::acquire_database_instance_lock;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const APP_DATA_ROOT_ENV: &str = "MYCOPILOT_APP_DATA_ROOT";
const STORAGE_DATABASE_ENV: &str = "MYCOPILOT_STORAGE_DB";
const STORAGE_DATABASE_FILE_NAME: &str = "storage.sqlite";

pub(crate) fn database_path() -> io::Result<PathBuf> {
    database_path_from_values(
        std::env::var_os(APP_DATA_ROOT_ENV).map(PathBuf::from),
        std::env::var_os(STORAGE_DATABASE_ENV).map(PathBuf::from),
        default_database_path(),
    )
}

pub(crate) fn database_path_from_values(
    app_data_root: Option<PathBuf>,
    standalone_database_override: Option<PathBuf>,
    standalone_default_database_path: Option<PathBuf>,
) -> io::Result<PathBuf> {
    if let Some(app_data_root) = app_data_root {
        let app_data_root = validate_authoritative_app_data_root(&app_data_root)?;
        return Ok(app_data_root.join(STORAGE_DATABASE_FILE_NAME));
    }

    if let Some(database_path) = standalone_database_override {
        return Ok(database_path);
    }

    standalone_default_database_path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "cannot determine a default Core storage path because the platform user-data \
             environment is unavailable",
        )
    })
}

fn validate_authoritative_app_data_root(root: &Path) -> io::Result<PathBuf> {
    if root.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{APP_DATA_ROOT_ENV} cannot be empty"),
        ));
    }
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{APP_DATA_ROOT_ENV} must be an absolute path, received `{}`",
                root.display()
            ),
        ));
    }
    fs::create_dir_all(root)?;
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{APP_DATA_ROOT_ENV} must not name a symbolic link: {}",
                root.display()
            ),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{APP_DATA_ROOT_ENV} must name a directory: {}",
                root.display()
            ),
        ));
    }
    fs::canonicalize(root)
}

pub(crate) fn default_database_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return absolute_platform_directory(std::env::var_os("HOME")).map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("mycopilot-next")
                .join(STORAGE_DATABASE_FILE_NAME)
        });
    }

    if cfg!(target_os = "windows") {
        return absolute_platform_directory(std::env::var_os("APPDATA")).map(|app_data| {
            app_data
                .join("mycopilot-next")
                .join(STORAGE_DATABASE_FILE_NAME)
        });
    }

    if let Some(xdg_data_home) = absolute_platform_directory(std::env::var_os("XDG_DATA_HOME")) {
        return Some(
            xdg_data_home
                .join("mycopilot-next")
                .join(STORAGE_DATABASE_FILE_NAME),
        );
    }
    absolute_platform_directory(std::env::var_os("HOME")).map(|home| {
        home.join(".local")
            .join("share")
            .join("mycopilot-next")
            .join(STORAGE_DATABASE_FILE_NAME)
    })
}

fn absolute_platform_directory(value: Option<OsString>) -> Option<PathBuf> {
    value
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
}

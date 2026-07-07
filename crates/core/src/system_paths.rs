// Rust agent core.
use std::env;
use std::path::{Component, Path, PathBuf};

pub fn expand_system_path(input: &str) -> Result<Option<PathBuf>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }

    let (base, remainder) = if input == "~" {
        (home_dir()?, "")
    } else if let Some(remainder) = strip_alias_prefix(input, "~") {
        (home_dir()?, remainder)
    } else if input == "@home" {
        (home_dir()?, "")
    } else if let Some(remainder) = strip_alias_prefix(input, "@home") {
        (home_dir()?, remainder)
    } else if input == "@desktop" {
        (desktop_dir()?, "")
    } else if let Some(remainder) = strip_alias_prefix(input, "@desktop") {
        (desktop_dir()?, remainder)
    } else if input == "@documents" {
        (documents_dir()?, "")
    } else if let Some(remainder) = strip_alias_prefix(input, "@documents") {
        (documents_dir()?, remainder)
    } else if input == "@downloads" {
        (downloads_dir()?, "")
    } else if let Some(remainder) = strip_alias_prefix(input, "@downloads") {
        (downloads_dir()?, remainder)
    } else {
        return Ok(None);
    };

    Ok(Some(base.join(clean_alias_remainder(remainder)?)))
}

pub fn system_path_aliases() -> &'static [&'static str] {
    &["~", "@home", "@desktop", "@documents", "@downloads"]
}

fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            let drive = env::var_os("HOMEDRIVE")?;
            let path = env::var_os("HOMEPATH")?;
            Some(PathBuf::from(drive).join(path))
        })
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "无法确定当前用户主目录。".to_string())
}

fn desktop_dir() -> Result<PathBuf, String> {
    known_dir(dirs::desktop_dir(), "Desktop", "桌面")
}

fn documents_dir() -> Result<PathBuf, String> {
    known_dir(dirs::document_dir(), "Documents", "文档")
}

fn downloads_dir() -> Result<PathBuf, String> {
    known_dir(dirs::download_dir(), "Downloads", "下载")
}

fn known_dir(
    resolved: Option<PathBuf>,
    fallback_name: &str,
    label: &str,
) -> Result<PathBuf, String> {
    resolved
        .filter(|path| path.is_absolute())
        .or_else(|| home_dir().ok().map(|home| home.join(fallback_name)))
        .filter(|path| path.is_absolute())
        .ok_or_else(|| format!("无法确定当前用户的{label}目录。"))
}

fn strip_alias_prefix<'a>(input: &'a str, alias: &str) -> Option<&'a str> {
    input.strip_prefix(alias).and_then(|remainder| {
        remainder
            .strip_prefix('/')
            .or_else(|| remainder.strip_prefix('\\'))
    })
}

fn clean_alias_remainder(remainder: &str) -> Result<PathBuf, String> {
    let mut cleaned = PathBuf::new();
    for component in Path::new(remainder).components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("系统路径别名后不能包含 .. 或绝对路径。".to_string());
            }
        }
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home_and_desktop_aliases() {
        let home = expand_system_path("~").unwrap().unwrap();

        assert_eq!(expand_system_path("@home").unwrap().unwrap(), home);
        assert_eq!(
            expand_system_path("@desktop/notes.txt").unwrap().unwrap(),
            desktop_dir().unwrap().join("notes.txt")
        );
    }

    #[test]
    fn rejects_alias_parent_traversal() {
        assert!(expand_system_path("@desktop/../secret").is_err());
    }
}

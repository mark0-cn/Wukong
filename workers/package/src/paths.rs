use anyhow::{Result, ensure};

/// Package paths use a deliberately portable ASCII subset. Installation roots
/// may contain Unicode; archive members never depend on Windows case folding,
/// device aliases, alternate data streams or Win32 path normalization.
pub fn validate_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= 512 && path.is_ascii(),
        "invalid package path"
    );
    for component in path.split('/') {
        ensure!(
            !component.is_empty() && component.len() <= 200,
            "invalid package path component"
        );
        ensure!(
            component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._- ".contains(&byte)),
            "invalid package path characters"
        );
        ensure!(
            !component.ends_with(['.', ' ']) && !component.starts_with(' '),
            "ambiguous Windows package path"
        );
        let stem = component.split('.').next().unwrap().to_ascii_uppercase();
        ensure!(!stem.ends_with(' '), "ambiguous Windows package stem");
        ensure!(
            !["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str()),
            "Windows device path is forbidden"
        );
        ensure!(
            !(stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit()),
            "Windows device path is forbidden"
        );
    }
    Ok(())
}

pub(crate) fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

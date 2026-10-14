//! One SSH session and one main window per process. Never transport credentials.
use serde::Serialize;
use std::process::{Command, Stdio};
use xyz_core::ssh::ApiError;

const PROFILE_FLAG: &str = "--xyz-profile";
const NEW_FLAG: &str = "--xyz-new-connection";

#[derive(Clone, Default, Serialize)]
pub struct WindowLaunchTarget {
    pub profile_id: Option<String>,
    pub new_connection: bool,
}

fn valid_profile_id(value: &str) -> bool {
    // Profile IDs are UUIDs. Reject whitespace, options, paths and shell metacharacters.
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

pub fn parse_launch_target(
    args: impl IntoIterator<Item = String>,
) -> Result<WindowLaunchTarget, ApiError> {
    let mut args = args.into_iter();
    let Some(flag) = args.next() else {
        return Ok(WindowLaunchTarget::default());
    };
    let target = if flag == NEW_FLAG {
        WindowLaunchTarget {
            profile_id: None,
            new_connection: true,
        }
    } else if flag == PROFILE_FLAG {
        let id = args
            .next()
            .filter(|value| valid_profile_id(value))
            .ok_or_else(invalid_target)?;
        WindowLaunchTarget {
            profile_id: Some(id),
            new_connection: false,
        }
    } else {
        return Err(invalid_target());
    };
    if args.next().is_some() {
        return Err(invalid_target());
    }
    Ok(target)
}

fn invalid_target() -> ApiError {
    ApiError::new(
        "WINDOW_TARGET_INVALID",
        "The new window target is invalid. Open a new window and select a saved connection.",
    )
}

#[tauri::command]
pub fn get_window_launch_target(state: tauri::State<'_, WindowLaunchTarget>) -> WindowLaunchTarget {
    state.inner().clone()
}

#[tauri::command]
pub fn launch_new_window(profile_id: Option<String>) -> Result<(), ApiError> {
    if profile_id
        .as_deref()
        .is_some_and(|id| !valid_profile_id(id))
    {
        return Err(invalid_target());
    }
    let executable = std::env::current_exe().map_err(|_| launch_failed())?;
    let mut command = Command::new(executable);
    // Only safe persisted identifiers are added. No shell, URL, credential, or draft target.
    if let Some(id) = profile_id {
        command.arg(PROFILE_FLAG).arg(id);
    } else {
        command.arg(NEW_FLAG);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|_| launch_failed())?;
    // Reap on Unix without blocking the originating window or detaching its SSH.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn launch_failed() -> ApiError {
    ApiError::new(
        "WINDOW_LAUNCH_FAILED",
        "A new application process could not be started. Your current SSH session is unchanged.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_new_window_or_a_persisted_uuid() {
        assert!(
            !parse_launch_target(Vec::<String>::new())
                .unwrap()
                .new_connection
        );
        assert!(
            parse_launch_target([NEW_FLAG.to_owned()])
                .unwrap()
                .new_connection
        );
        let id = "01234567-89ab-cdef-0123-456789abcdef";
        assert_eq!(
            parse_launch_target([PROFILE_FLAG.to_owned(), id.to_owned()])
                .unwrap()
                .profile_id
                .as_deref(),
            Some(id)
        );
        for args in [
            vec!["--password", "secret"],
            vec![PROFILE_FLAG, "../profile"],
            vec![PROFILE_FLAG, id, "secret"],
            vec![NEW_FLAG, "host"],
            vec![PROFILE_FLAG],
        ] {
            assert!(parse_launch_target(args.into_iter().map(str::to_owned)).is_err());
        }
    }
}

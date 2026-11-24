use std::process::Command;

/// Apply the hosted worker's unprivileged identity to verification subprocesses.
///
/// Local CLI checks do not set these variables and retain their existing
/// behavior. Hosted workers run as a small trusted supervisor and set both
/// values before executing untrusted project tests or proof tools.
pub fn apply_hosted_identity(command: &mut Command) {
    apply_hosted_identity_values(
        command,
        env_id("FORALL_SANDBOX_UID"),
        env_id("FORALL_SANDBOX_GID"),
    );
}

fn apply_hosted_identity_values(command: &mut Command, uid: Option<u32>, gid: Option<u32>) {
    if uid.is_some() {
        for name in [
            "FORALL_JOB_INPUT_URL",
            "FORALL_JOB_OUTPUT_URL",
            "FORALL_JOB_SOURCE_URL",
            "FORALL_MCP_API_KEYS",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
        ] {
            command.env_remove(name);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        if let Some(gid) = gid {
            command.gid(gid);
        }
        if let Some(uid) = uid {
            command.uid(uid);
        }
    }
}

#[cfg(unix)]
fn env_id(name: &str) -> Option<u32> {
    std::env::var(name).ok()?.parse().ok()
}

#[cfg(not(unix))]
fn env_id(_name: &str) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_identity_keeps_local_command_unchanged() {
        let mut command = Command::new("true");
        apply_hosted_identity(&mut command);
        assert_eq!(command.get_program(), "true");
    }

    #[test]
    fn hosted_identity_removes_job_urls_and_cloud_credentials() {
        let mut command = Command::new("true");
        command
            .env("FORALL_JOB_INPUT_URL", "https://signed.example/input")
            .env("FORALL_JOB_SOURCE_URL", "https://signed.example/source")
            .env("AWS_ACCESS_KEY_ID", "secret");
        apply_hosted_identity_values(&mut command, Some(10001), Some(10001));
        let environment = command
            .get_envs()
            .map(|(name, value)| (name.to_string_lossy().into_owned(), value.is_none()))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(environment.get("FORALL_JOB_INPUT_URL"), Some(&true));
        assert_eq!(environment.get("FORALL_JOB_SOURCE_URL"), Some(&true));
        assert_eq!(environment.get("AWS_ACCESS_KEY_ID"), Some(&true));
    }
}

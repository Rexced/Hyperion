//! One-time root setup for sensors that Linux keeps root-only. Runs as
//! `hyperion --grant-sensors cpu-power drivetemp` under sudo (installer) or pkexec
//! (the Settings button), does only the fixed steps below, then exits.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

pub const FLAG: &str = "--grant-sensors";

const RAPL_RULE: &str = "/etc/udev/rules.d/60-hyperion-rapl.rules";
const DRIVETEMP_CONF: &str = "/etc/modules-load.d/hyperion-drivetemp.conf";

/// The rule letting group `gid` read the CPU energy counters, now and after reboots.
fn rapl_rule(gid: u32) -> String {
    format!(
        "# Written by Hyperion. Lets group {gid} read CPU package power (RAPL energy),\n\
         # which Linux otherwise restricts to root. Delete this file to undo.\n\
         SUBSYSTEM==\"powercap\", KERNEL==\"intel-rapl:*\", \
         RUN+=\"/bin/chgrp {gid} /sys%p/energy_uj\", RUN+=\"/bin/chmod g+r /sys%p/energy_uj\"\n"
    )
}

/// Primary group of the user who ran sudo/pkexec (not root's).
fn invoking_gid() -> Option<u32> {
    if let Some(gid) = std::env::var("SUDO_GID").ok().and_then(|g| g.parse().ok()) {
        return Some(gid);
    }
    let uid: u32 = std::env::var("PKEXEC_UID").ok()?.parse().ok()?;
    // SAFETY: getpwuid returns null or a pointer to static storage, valid until the
    // next call; we copy the one field we need immediately. Single-threaded here.
    let pw = unsafe { libc::getpwuid(uid) };
    (!pw.is_null()).then(|| unsafe { (*pw).pw_gid })
}

fn grant_cpu_power(gid: u32) -> Result<(), String> {
    std::fs::write(RAPL_RULE, rapl_rule(gid)).map_err(|e| format!("writing {RAPL_RULE}: {e}"))?;
    // Apply now as well, so it works without a reboot.
    let entries = std::fs::read_dir("/sys/class/powercap").map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("intel-rapl:") {
            continue;
        }
        let file = entry.path().join("energy_uj");
        if !file.exists() {
            continue;
        }
        std::os::unix::fs::chown(&file, None, Some(gid))
            .map_err(|e| format!("{}: {e}", file.display()))?;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o440))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(())
}

fn enable_drivetemp() -> Result<(), String> {
    std::fs::write(DRIVETEMP_CONF, "drivetemp\n")
        .map_err(|e| format!("writing {DRIVETEMP_CONF}: {e}"))?;
    let status = Command::new("modprobe")
        .arg("drivetemp")
        .status()
        .map_err(|e| format!("running modprobe: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "modprobe drivetemp failed".to_owned())
}

/// Entry point for `--grant-sensors`; returns the process exit code.
pub fn run(what: &[String]) -> i32 {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("hyperion: {FLAG} must run as root (via sudo or pkexec)");
        return 1;
    }
    let Some(gid) = invoking_gid() else {
        eprintln!("hyperion: run this through sudo or pkexec so it knows which user to allow");
        return 1;
    };
    if what.is_empty() {
        eprintln!("hyperion: usage: {FLAG} [cpu-power] [drivetemp]");
        return 1;
    }
    for item in what {
        let result = match item.as_str() {
            "cpu-power" => grant_cpu_power(gid),
            "drivetemp" => enable_drivetemp(),
            other => Err(format!("unknown item `{other}`")),
        };
        if let Err(e) = result {
            eprintln!("hyperion: {e}");
            return 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_only_grants_read_to_the_given_group() {
        let rule = rapl_rule(1000);
        assert!(rule.contains("KERNEL==\"intel-rapl:*\""));
        assert!(rule.contains("/bin/chgrp 1000 /sys%p/energy_uj"));
        assert!(rule.contains("/bin/chmod g+r /sys%p/energy_uj"));
        assert!(!rule.contains("o+r"));
    }
}

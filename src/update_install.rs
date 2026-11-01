//! Stages a verified release beside the installed app, then hands replacement to
//! a small detached helper after this process releases its profile lock.
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

const MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

struct InstallTarget {
    destination: PathBuf,
    executable: PathBuf,
}

pub struct PreparedInstall {
    work: PathBuf,
    destination: PathBuf,
    payload: PathBuf,
    executable: PathBuf,
    backup: PathBuf,
    arguments: Vec<std::ffi::OsString>,
    launched: bool,
}

impl Drop for PreparedInstall {
    fn drop(&mut self) {
        if !self.launched {
            let _ = fs::remove_dir_all(&self.work);
        }
    }
}

/// System-managed installations and loose development binaries use the manual
/// download flow. A portable folder must carry the packaging marker.
pub fn can_install() -> bool {
    install_target().is_ok_and(|target| {
        target
            .destination
            .parent()
            .is_some_and(|parent| writable_parent(parent).is_ok())
    })
}

fn install_target() -> Result<InstallTarget> {
    let executable = std::env::current_exe()?.canonicalize()?;
    #[cfg(target_os = "macos")]
    {
        let macos = executable.parent().context("Missing application folder")?;
        let contents = macos.parent().context("Missing application contents")?;
        let bundle = contents.parent().context("Missing application bundle")?;
        ensure!(
            macos.file_name().is_some_and(|v| v == "MacOS")
                && contents.file_name().is_some_and(|v| v == "Contents")
                && bundle.extension().is_some_and(|v| v == "app")
                && bundle.join("Contents/Info.plist").is_file(),
            "Open the packaged app to install updates, or download the release manually"
        );
        Ok(InstallTarget {
            destination: bundle.to_owned(),
            executable: PathBuf::from("Contents/MacOS/clipclub"),
        })
    }
    #[cfg(target_os = "linux")]
    {
        let folder = executable.parent().context("Missing application folder")?;
        ensure!(
            !["/usr", "/opt", "/bin", "/sbin", "/snap", "/var/lib"]
                .iter()
                .any(|path| folder.starts_with(path)),
            "Use your package manager or download the release to update this installation"
        );
        let marker: serde_json::Value = serde_json::from_reader(
            File::open(folder.join("BUILD-INFO.json"))
                .context("Automatic installation requires the portable release folder")?,
        )?;
        ensure!(
            executable.file_name().is_some_and(|v| v == "clipclub")
                && folder.join("launch-clipclub").is_file()
                && folder.join("THIRD_PARTY.md").is_file()
                && marker["application"] == "clipclub"
                && marker["target"] == "x86_64-unknown-linux-gnu",
            "Automatic installation requires the portable release folder"
        );
        Ok(InstallTarget {
            destination: folder.to_owned(),
            executable: PathBuf::from("clipclub"),
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = executable;
        anyhow::bail!("Download the release to update this platform")
    }
}

fn writable_parent(parent: &Path) -> Result<()> {
    // Test actual access instead of permission bits (ACLs may also apply).
    let probe = parent.join(format!(
        ".clipclub-write-{}",
        hex::encode(rand::random::<[u8; 16]>())
    ));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .context("The application folder is read-only; move the app to a writable folder first")?;
    fs::remove_file(probe)?;
    Ok(())
}

pub fn prepare(archive: &Path, version: &str, profile: &Path) -> Result<PreparedInstall> {
    let target = install_target()?;
    let profile = profile.canonicalize()?;
    ensure!(
        !profile.starts_with(&target.destination),
        "This profile is inside the application folder; move it outside before installing an update"
    );
    let parent = target
        .destination
        .parent()
        .context("Missing installation parent")?
        .to_owned();
    writable_parent(&parent)?;
    let suffix = hex::encode(rand::random::<[u8; 16]>());
    let work = parent.join(format!(".clipclub-update-{suffix}"));
    fs::create_dir(&work)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&work, fs::Permissions::from_mode(0o700))?;
    }
    let mut prepared = PreparedInstall {
        work,
        destination: target.destination,
        payload: PathBuf::new(),
        executable: target.executable,
        backup: parent.join(format!(".clipclub-backup-{suffix}")),
        arguments: std::env::args_os().skip(1).collect(),
        launched: false,
    };
    let unpacked = prepared.work.join("payload");
    fs::create_dir(&unpacked)?;
    #[cfg(target_os = "macos")]
    {
        extract_zip(archive, &unpacked, "clipclub.app")?;
        prepared.payload = unpacked.join("clipclub.app");
        let plist = prepared.payload.join("Contents/Info.plist");
        let result = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleShortVersionString"])
            .arg(&plist)
            .output()?;
        ensure!(
            result.status.success()
                && String::from_utf8_lossy(&result.stdout).trim()
                    == version.split('+').next().unwrap_or(version),
            "The downloaded app version does not match its release"
        );
        ensure!(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(&prepared.payload)
                .output()?
                .status
                .success(),
            "The downloaded app signature is invalid"
        );
    }
    #[cfg(target_os = "linux")]
    {
        let name = format!("clipclub-{version}-linux-x86_64");
        extract_tar(archive, &unpacked, &name)?;
        prepared.payload = unpacked.join(&name);
        let marker: serde_json::Value =
            serde_json::from_reader(File::open(prepared.payload.join("BUILD-INFO.json"))?)?;
        ensure!(
            marker["application"] == "clipclub"
                && marker["version"] == version
                && marker["target"] == "x86_64-unknown-linux-gnu",
            "The downloaded package does not match its release and platform"
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    anyhow::bail!("Automatic installation is unavailable on this platform");
    let binary = prepared.payload.join(&prepared.executable);
    ensure!(
        binary.is_file(),
        "The release is missing the application executable"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            fs::metadata(&binary)?.permissions().mode() & 0o111 != 0,
            "The release executable is not runnable"
        );
    }
    validate_binary(&binary, std::env::consts::OS, std::env::consts::ARCH)?;
    prepared.arguments.push("--data-dir".into());
    prepared.arguments.push(profile.into_os_string());
    Ok(prepared)
}

fn validate_binary(path: &Path, operating_system: &str, architecture: &str) -> Result<()> {
    let mut header = [0_u8; 20];
    File::open(path)?
        .read_exact(&mut header)
        .context("Invalid release executable")?;
    let valid = match (operating_system, architecture) {
        ("macos", "aarch64" | "x86_64") => {
            let expected_cpu = if architecture == "aarch64" {
                0x0100000c
            } else {
                0x01000007
            };
            header[..4] == [0xcf, 0xfa, 0xed, 0xfe]
                && u32::from_le_bytes(header[4..8].try_into().unwrap()) == expected_cpu
                && u32::from_le_bytes(header[12..16].try_into().unwrap()) == 2
        }
        ("linux", "x86_64") => {
            header[..6] == [0x7f, b'E', b'L', b'F', 2, 1]
                && matches!(
                    u16::from_le_bytes(header[16..18].try_into().unwrap()),
                    2 | 3
                )
                && u16::from_le_bytes(header[18..20].try_into().unwrap()) == 62
        }
        _ => false,
    };
    ensure!(
        valid,
        "The release executable does not match this operating system and CPU"
    );
    Ok(())
}

impl PreparedInstall {
    /// Launches without a shell command string: paths and restart arguments are
    /// passed as positional parameters and never interpreted as shell source.
    pub fn launch(mut self) -> Result<()> {
        let script = self.work.join("install.sh");
        fs::write(&script, INSTALL_HELPER)?;
        let log = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.work.join("install.log"))?;
        let mut command = Command::new("/bin/sh");
        command
            .arg(&script)
            .arg(std::process::id().to_string())
            .arg(&self.destination)
            .arg(&self.payload)
            .arg(&self.backup)
            .arg(&self.executable)
            .arg(&self.work)
            .args(&self.arguments)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        command
            .spawn()
            .context("Could not start the update installer")?;
        self.launched = true;
        Ok(())
    }
}

const INSTALL_HELPER: &str = r#"#!/bin/sh
set -eu
old_pid=$1
destination=$2
payload=$3
backup=$4
executable=$5
work=$6
shift 6
attempt=0
while kill -0 "$old_pid" 2>/dev/null; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 120 ]; then
    echo 'The app did not close; the original installation was retained.'
    exit 1
  fi
  /bin/sleep 1
done
# Do not follow a changed target or overwrite an existing backup.
if [ ! -d "$destination" ] || [ -L "$destination" ] || [ ! -d "$payload" ] || [ -L "$payload" ] || [ -e "$backup" ]; then
  echo 'The installation changed while preparing the update.'
  exit 1
fi
/bin/mv "$destination" "$backup"
if ! /bin/mv "$payload" "$destination"; then
  /bin/mv "$backup" "$destination"
  echo 'The original installation was restored.'
  exit 1
fi
/usr/bin/nohup "$destination/$executable" "$@" </dev/null >>"$work/restart.log" 2>&1 &
new_pid=$!
/bin/sleep 3
if ! kill -0 "$new_pid" 2>/dev/null; then
  # A startup failure restores the complete old package before restarting it.
  /bin/mv "$destination" "$work/failed-app"
  /bin/mv "$backup" "$destination"
  /usr/bin/nohup "$destination/$executable" "$@" </dev/null >>"$work/restart.log" 2>&1 &
  echo 'The updated app failed to start; the previous version was restored.'
  exit 1
fi
# Retain the old package and logs for manual recovery. The next successful
# update gets its own unique backup rather than removing another version.
echo "Update installed. Previous version: $backup"
"#;

fn entry_path(name: &Path, root: &str) -> Result<PathBuf> {
    let text = name
        .to_str()
        .context("The archive contains a non-UTF-8 path")?;
    ensure!(
        text.len() <= 4096 && !text.contains(['\\', '\0', ':']),
        "Invalid archive path"
    );
    let mut parts = name.components();
    ensure!(
        matches!(parts.next(), Some(Component::Normal(part)) if part == root),
        "The archive has an unexpected root folder"
    );
    ensure!(
        parts.all(|part| matches!(part, Component::Normal(_))),
        "The archive contains an unsafe path"
    );
    Ok(name.to_owned())
}

fn copy_entry(mut input: impl Read, path: &Path, size: u64, mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = OpenOptions::new().create_new(true).write(true).open(path)?;
    let copied = io::copy(&mut (&mut input).take(size + 1), &mut output)?;
    ensure!(copied == size, "The archive entry has an invalid size");
    output.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if mode & 0o111 != 0 { 0o755 } else { 0o644 }),
        )?;
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn extract_zip(archive: &Path, destination: &Path, root: &str) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(archive)?)?;
    ensure!(
        archive.len() <= MAX_ENTRIES,
        "The release archive contains too many entries"
    );
    let mut expanded = 0_u64;
    let mut seen = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let path = entry_path(Path::new(entry.name()), root)?;
        ensure!(seen.insert(path.clone()), "The archive repeats a path");
        let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
        ensure!(
            kind == 0 || kind == 0o100000 || (kind == 0o040000 && entry.is_dir()),
            "Archive links and special files are not allowed"
        );
        expanded = expanded
            .checked_add(entry.size())
            .context("Invalid archive size")?;
        ensure!(
            expanded <= MAX_EXPANDED_BYTES,
            "The release archive expands beyond the size limit"
        );
        let path = destination.join(path);
        if entry.is_dir() {
            fs::create_dir_all(path)?;
        } else {
            let size = entry.size();
            let mode = entry.unix_mode().unwrap_or(0o644);
            copy_entry(&mut entry, &path, size, mode)?;
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn extract_tar(archive: &Path, destination: &Path, root: &str) -> Result<()> {
    let compressed = flate2::read::GzDecoder::new(File::open(archive)?);
    // Bound decompression including extended-name metadata processed by tar.
    let mut archive =
        tar::Archive::new(compressed.take(MAX_EXPANDED_BYTES + MAX_ENTRIES as u64 * 2048));
    let mut expanded = 0_u64;
    let mut seen = BTreeSet::new();
    for (index, entry) in archive.entries()?.enumerate() {
        ensure!(
            index < MAX_ENTRIES,
            "The release archive contains too many entries"
        );
        let mut entry = entry?;
        let path = entry_path(&entry.path()?, root)?;
        ensure!(seen.insert(path.clone()), "The archive repeats a path");
        let kind = entry.header().entry_type();
        ensure!(
            kind.is_file() || kind.is_dir(),
            "Archive links and special files are not allowed"
        );
        let size = entry.header().size()?;
        expanded = expanded.checked_add(size).context("Invalid archive size")?;
        ensure!(
            expanded <= MAX_EXPANDED_BYTES,
            "The release archive expands beyond the size limit"
        );
        let path = destination.join(path);
        if kind.is_dir() {
            fs::create_dir_all(path)?;
        } else {
            let mode = entry.header().mode()?;
            copy_entry(&mut entry, &path, size, mode)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn native_binary_headers_reject_wrong_operating_system_and_cpu() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("clipclub");
        let mut macho = [0_u8; 20];
        macho[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        macho[4..8].copy_from_slice(&0x0100000c_u32.to_le_bytes());
        macho[12..16].copy_from_slice(&2_u32.to_le_bytes());
        fs::write(&binary, macho).unwrap();
        assert!(validate_binary(&binary, "macos", "aarch64").is_ok());
        assert!(validate_binary(&binary, "macos", "x86_64").is_err());
        assert!(validate_binary(&binary, "linux", "x86_64").is_err());
        let mut elf = [0_u8; 20];
        elf[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1]);
        elf[16..18].copy_from_slice(&3_u16.to_le_bytes());
        elf[18..20].copy_from_slice(&62_u16.to_le_bytes());
        fs::write(&binary, elf).unwrap();
        assert!(validate_binary(&binary, "linux", "x86_64").is_ok());
        assert!(validate_binary(&binary, "macos", "aarch64").is_err());
        elf[18..20].copy_from_slice(&183_u16.to_le_bytes());
        fs::write(&binary, elf).unwrap();
        assert!(validate_binary(&binary, "linux", "x86_64").is_err());
    }

    #[test]
    fn paths_cannot_escape_or_select_another_application() {
        for name in [
            "../clipclub.app/file",
            "/clipclub.app/file",
            "other.app/file",
            "clipclub.app/../file",
            "clipclub.app/a\\b",
            "clipclub.app/C:file",
        ] {
            assert!(
                entry_path(Path::new(name), "clipclub.app").is_err(),
                "{name}"
            );
        }
        assert!(
            entry_path(
                Path::new("clipclub.app/Contents/MacOS/clipclub"),
                "clipclub.app"
            )
            .is_ok()
        );
    }

    #[test]
    fn zip_rejects_symlinks_and_preserves_executable_mode() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("release.zip");
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer
            .start_file(
                "clipclub.app/Contents/MacOS/clipclub",
                zip::write::SimpleFileOptions::default().unix_permissions(0o755),
            )
            .unwrap();
        writer.write_all(b"test executable").unwrap();
        writer.finish().unwrap();
        extract_zip(&archive, &output, "clipclub.app").unwrap();
        assert_eq!(
            fs::read(output.join("clipclub.app/Contents/MacOS/clipclub")).unwrap(),
            b"test executable"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                fs::metadata(output.join("clipclub.app/Contents/MacOS/clipclub"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer
            .add_symlink(
                "clipclub.app/link",
                "/outside",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.finish().unwrap();
        assert!(extract_zip(&archive, &temp.path().join("hostile"), "clipclub.app").is_err());
    }

    #[test]
    fn tar_rejects_links_and_duplicate_files() {
        let temp = tempfile::tempdir().unwrap();
        for link in [false, true] {
            let path = temp.path().join(if link {
                "link.tar.gz"
            } else {
                "duplicate.tar.gz"
            });
            let compressed = flate2::write::GzEncoder::new(
                File::create(&path).unwrap(),
                flate2::Compression::default(),
            );
            let mut writer = tar::Builder::new(compressed);
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o644);
            if link {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("/outside").unwrap();
            }
            header.set_cksum();
            writer
                .append_data(&mut header, "portable/clipclub", io::empty())
                .unwrap();
            if !link {
                writer
                    .append_data(&mut header, "portable/clipclub", io::empty())
                    .unwrap();
            }
            writer.into_inner().unwrap().finish().unwrap();
            assert!(
                extract_tar(
                    &path,
                    &temp.path().join(if link { "links" } else { "duplicates" }),
                    "portable"
                )
                .is_err()
            );
        }
    }

    #[cfg(unix)]
    fn fake_executable(path: &Path, fails: bool) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, if fails {
            "#!/bin/sh\nexit 17\n"
        } else {
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$2\"\necho \"$$\" > \"$1\"\nexec /bin/sleep 20\n"
        }).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn helper_installs_or_rolls_back_and_passes_arguments_as_data() {
        for fails in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let destination = temp.path().join("app with spaces and ' quotes");
            let work = temp.path().join("work");
            let payload = work.join("payload");
            let backup = temp.path().join("backup");
            let pidfile = temp.path().join("started.pid");
            let arguments = temp.path().join("arguments.txt");
            fs::create_dir(&work).unwrap();
            fake_executable(&destination.join("clipclub"), false);
            fs::write(destination.join("original-marker"), b"old").unwrap();
            fake_executable(&payload.join("clipclub"), fails);
            fs::write(payload.join("updated-marker"), b"new").unwrap();
            let script = work.join("install.sh");
            fs::write(&script, INSTALL_HELPER).unwrap();
            let literal = "$(touch should-not-exist); ' untouched";
            let result = Command::new("/bin/sh")
                .arg(&script)
                .args([
                    std::ffi::OsStr::new("2147483647"),
                    destination.as_os_str(),
                    payload.as_os_str(),
                    backup.as_os_str(),
                    std::ffi::OsStr::new("clipclub"),
                    work.as_os_str(),
                    pidfile.as_os_str(),
                    arguments.as_os_str(),
                    std::ffi::OsStr::new(literal),
                ])
                .output()
                .unwrap();
            for _ in 0..20 {
                if pidfile.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let pid = fs::read_to_string(&pidfile).unwrap();
            let _ = Command::new("/bin/kill").arg(pid.trim()).status();
            let received = fs::read_to_string(&arguments).unwrap();
            assert!(received.lines().any(|line| line == literal), "{received}");
            assert_eq!(
                result.status.success(),
                !fails,
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(destination.join("original-marker").exists(), fails);
            assert_eq!(destination.join("updated-marker").exists(), !fails);
            assert_eq!(backup.exists(), !fails);
            assert!(!temp.path().join("should-not-exist").exists());
        }
    }

    #[test]
    fn failed_preparation_removes_its_staging_directory() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        {
            let _prepared = PreparedInstall {
                work: work.clone(),
                destination: temp.path().join("app"),
                payload: PathBuf::new(),
                executable: PathBuf::new(),
                backup: PathBuf::new(),
                arguments: vec![],
                launched: false,
            };
        }
        assert!(!work.exists());
    }
}

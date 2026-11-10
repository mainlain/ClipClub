#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
mod app;
mod core;
mod crypto;
mod diagnostic_inbox;
mod diagnostics;
mod discovery;
mod http;
mod media;
mod peer;
pub mod rating_distribution;
mod torrent;
mod update;
mod update_install;
mod wallet;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    time::Duration,
};

struct Options {
    root: PathBuf,
    p2p: u16,
    torrent: u16,
    http: u16,
    headless: bool,
}

#[cfg(any(target_os = "macos", test))]
fn macos_data_dir(home: Option<std::ffi::OsString>) -> Result<PathBuf> {
    let home = PathBuf::from(home.context("Cannot locate home folder; use --data-dir")?);
    anyhow::ensure!(home.is_absolute(), "Invalid home folder; use --data-dir");
    // Explicit --user-data mode keeps state in the platform's user data folder.
    Ok(home.join("Library/Application Support/ClipCrop Points"))
}

#[cfg(any(target_os = "linux", test))]
fn linux_data_dir(
    data_home: Option<std::ffi::OsString>,
    user_home: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    // Ignore empty or relative XDG paths rather than writing beside the app.
    if let Some(directory) = data_home.map(PathBuf::from).filter(|p| p.is_absolute()) {
        return Ok(directory.join("clipcrop-points"));
    }
    let directory = PathBuf::from(user_home.context("Cannot locate home folder; use --data-dir")?);
    anyhow::ensure!(
        directory.is_absolute(),
        "Invalid home folder; use --data-dir"
    );
    Ok(directory.join(".local/share/clipcrop-points"))
}

fn portable_data_dir(executable: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        executable.is_absolute(),
        "Cannot locate the application; use --data-dir"
    );
    let parent = executable
        .parent()
        .context("Cannot locate executable folder")?;
    // Keep data outside the signed bundle, so signing and app replacement only
    // touch application files. The bundle and data still travel in one folder.
    let application_parent = parent
        .parent()
        .filter(|contents| {
            parent.file_name().is_some_and(|v| v == "MacOS")
                && contents.file_name().is_some_and(|v| v == "Contents")
        })
        .and_then(|contents| contents.parent())
        .filter(|bundle| bundle.extension().is_some_and(|v| v == "app"))
        .and_then(|bundle| bundle.parent())
        .unwrap_or(parent);
    Ok(application_parent.join("clipclub-data"))
}

fn default_data_dir(user_data: bool) -> Result<PathBuf> {
    if !user_data {
        return portable_data_dir(&std::env::current_exe()?);
    }
    #[cfg(target_os = "macos")]
    {
        macos_data_dir(std::env::var_os("HOME"))
    }
    #[cfg(target_os = "linux")]
    {
        linux_data_dir(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        anyhow::bail!(
            "--user-data is supported on macOS and Linux; use --data-dir on this platform"
        )
    }
}

fn parse_options(
    args: impl IntoIterator<Item = String>,
    default_root: impl FnOnce(bool) -> Result<PathBuf>,
) -> Result<Option<Options>> {
    let mut root = None;
    let mut user_data = false;
    let mut options = Options {
        root: PathBuf::new(),
        p2p: 42931,
        torrent: 42932,
        http: 0,
        headless: false,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "clipclub — a peer-to-peer video commons\n\nBy default, your profile is in clipclub-data beside the executable or macOS app bundle.\nMove the app and clipclub-data together.\n\nOptions:\n  --data-dir PATH       Use a specific profile folder (overrides --user-data)\n  --user-data           Store the profile in the macOS/Linux user data folder\n  --p2p-port PORT       Peer mesh TCP port (42931)\n  --torrent-port PORT   BitTorrent TCP port (42932)\n  --http-port PORT      Local interface port (random by default)\n  --headless            Serve the local interface without a desktop window"
                );
                return Ok(None);
            }
            "--data-dir" => {
                root = Some(PathBuf::from(
                    args.next().context("--data-dir needs a path")?,
                ))
            }
            "--user-data" => user_data = true,
            "--p2p-port" => options.p2p = args.next().context("Missing peer port")?.parse()?,
            "--torrent-port" => {
                options.torrent = args.next().context("Missing torrent port")?.parse()?
            }
            "--http-port" => {
                options.http = args.next().context("Missing interface port")?.parse()?
            }
            "--headless" => options.headless = true,
            _ => anyhow::bail!("Unknown option: {arg}"),
        }
    }
    options.root = match root {
        Some(root) => root,
        None => default_root(user_data)?,
    };
    Ok(Some(options))
}

fn options() -> Result<Option<Options>> {
    let Some(mut options) = parse_options(std::env::args().skip(1), default_data_dir)? else {
        return Ok(None);
    };
    std::fs::create_dir_all(&options.root).with_context(|| format!(
        "Cannot create profile folder {}. Move the app and clipclub-data to a writable folder, or use --user-data or --data-dir PATH",
        options.root.display()
    ))?;
    options.root = options.root.canonicalize()?;
    Ok(Some(options))
}
fn run() -> Result<()> {
    let Some(options) = options()? else {
        return Ok(());
    };
    // Lock before loading the key or wallet; two clients must never spend the same profile.
    let lock_path = options.root.join("profile.lock");
    let meta = std::fs::symlink_metadata(&lock_path);
    if let Ok(meta) = meta {
        anyhow::ensure!(meta.is_file(), "Invalid profile lock file");
    }
    let profile_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!(
            "Cannot write to profile folder {}. Move the app and clipclub-data to a writable folder, or use --user-data or --data-dir PATH",
            options.root.display()
        ))?;
    profile_lock
        .try_lock_exclusive()
        .context("This profile is already open in another clipclub process")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&options.root, std::fs::Permissions::from_mode(0o700))?;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let app = runtime.block_on(app::App::start(options.root, options.p2p, options.torrent))?;
    let (url, _server) = runtime.block_on(http::serve(app.clone(), options.http))?;
    println!(
        "clipclub interface: {url}\nPeer identity: {}\nProfile folder: {}",
        app.mesh.peer_id,
        app.root.display()
    );
    if options.headless {
        runtime.block_on(async {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => (),
                _ = app.shutdown.cancelled() => (),
            }
            app.stop().await;
        });
    } else {
        #[cfg(feature = "desktop")]
        desktop(&url, &runtime, app.clone())?;
        #[cfg(not(feature = "desktop"))]
        anyhow::bail!(
            "This build has no desktop window; run with --headless or build with the desktop feature"
        );
    }
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(5), app.stop()).await;
    });
    drop(profile_lock);
    Ok(())
}
#[cfg(feature = "desktop")]
fn desktop(
    url: &str,
    runtime: &tokio::runtime::Runtime,
    app: std::sync::Arc<app::App>,
) -> Result<()> {
    use tao::{
        dpi::LogicalSize,
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        window::WindowBuilder,
    };
    let event_loop = EventLoop::new();
    let update_proxy = event_loop.create_proxy();
    let shutdown = app.shutdown.clone();
    runtime.spawn(async move {
        shutdown.cancelled().await;
        let _ = update_proxy.send_event(());
    });
    let window = WindowBuilder::new()
        .with_title("clipclub")
        .with_inner_size(LogicalSize::new(480.0, 620.0))
        .with_min_inner_size(LogicalSize::new(360.0, 480.0))
        .build(&event_loop)?;
    let origin = format!("{url}/");
    let builder = wry::WebViewBuilder::new()
        .with_url(url)
        // Durable state lives in the portable profile, not an OS browser store.
        .with_incognito(true)
        .with_autoplay(true)
        .with_devtools(cfg!(debug_assertions))
        .with_navigation_handler(move |target| target.starts_with(&origin));
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(&window)?;
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        // Tao owns the window's sole child; pack the WebView into that box.
        let container = window
            .default_vbox()
            .context("Missing GTK content container")?;
        builder.build_gtk(container)?
    };
    // Tao's loop does not return; use runtime handle and explicitly stop both torrent sessions on quit.
    let handle = runtime.handle().clone();
    event_loop.run(move |event, _, control_flow| {
        let _ = &webview;
        *control_flow = ControlFlow::Wait;
        if app.shutdown.is_cancelled()
            || matches!(
                event,
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                }
            )
        {
            handle.block_on(async {
                let _ = tokio::time::timeout(Duration::from_secs(5), app.stop()).await;
            });
            *control_flow = ControlFlow::Exit;
        }
    });
}
fn main() {
    if let Err(error) = run() {
        eprintln!("clipclub: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_profile_does_not_resolve_the_default_folder() {
        let options = parse_options(
            ["--data-dir", "chosen-profile", "--headless"].map(String::from),
            |_| panic!("An explicit profile must not inspect the default location"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.root, PathBuf::from("chosen-profile"));
        assert!(options.headless);
    }

    #[test]
    fn help_does_not_resolve_a_profile_folder() {
        assert!(
            parse_options(["--help".to_owned()], |_| {
                panic!("Help must not require access to a profile folder")
            })
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn portable_profile_travels_with_the_executable_or_bundle() {
        let directory = tempfile::tempdir().unwrap();
        for folder in ["original", "relocated folder"] {
            let install = directory.path().join(folder);
            for executable in [
                install.join("clipclub"),
                install.join("clipclub.exe"),
                install.join("Renamed.app/Contents/MacOS/clipclub"),
            ] {
                let options = parse_options([], |user_data| {
                    assert!(!user_data);
                    portable_data_dir(&executable)
                })
                .unwrap()
                .unwrap();
                assert_eq!(options.root, install.join("clipclub-data"));
                assert!(!options.root.exists(), "Parsing must not create a profile");
            }
        }
        assert!(portable_data_dir(Path::new("relative/clipclub")).is_err());
    }

    #[test]
    fn ordinary_nested_folders_are_not_treated_as_app_bundles() {
        let directory = tempfile::tempdir().unwrap();
        for folder in [
            "tools/Contents/MacOS",
            "tools.app/Other/MacOS",
            "tools.app/Contents/bin",
        ] {
            let folder = directory.path().join(folder);
            assert_eq!(
                portable_data_dir(&folder.join("clipclub")).unwrap(),
                folder.join("clipclub-data")
            );
        }
    }

    #[test]
    fn explicit_profile_overrides_user_data_in_either_argument_order() {
        for args in [
            ["--user-data", "--data-dir", "chosen-profile"],
            ["--data-dir", "chosen-profile", "--user-data"],
        ] {
            let options = parse_options(args.map(String::from), |_| {
                panic!("An explicit profile must not resolve user data")
            })
            .unwrap()
            .unwrap();
            assert_eq!(options.root, PathBuf::from("chosen-profile"));
        }
    }

    #[test]
    fn linux_profile_uses_absolute_xdg_data_home_without_requiring_home() {
        let data = tempfile::tempdir().unwrap();
        let options = parse_options(["--user-data".to_owned()], |user_data| {
            assert!(user_data);
            linux_data_dir(Some(data.path().into()), None)
        })
        .unwrap()
        .unwrap();
        assert_eq!(options.root, data.path().join("clipcrop-points"));
        assert!(!options.root.exists(), "Parsing must not create a profile");
    }

    #[test]
    fn linux_profile_defaults_to_user_data_for_unset_empty_or_relative_xdg_paths() {
        let user = tempfile::tempdir().unwrap();
        for data in [None, Some("".into()), Some("relative-data".into())] {
            assert_eq!(
                linux_data_dir(data, Some(user.path().into())).unwrap(),
                user.path().join(".local/share/clipcrop-points")
            );
        }
    }

    #[test]
    fn linux_missing_or_invalid_home_does_not_fall_back_to_a_portable_profile() {
        for user in [None, Some("".into()), Some("relative-home".into())] {
            assert!(linux_data_dir(None, user).is_err());
        }
    }

    #[test]
    fn macos_user_data_profile_is_in_application_support() {
        let home = tempfile::tempdir().unwrap();
        let options = parse_options(["--user-data".to_owned()], |user_data| {
            assert!(user_data);
            macos_data_dir(Some(home.path().into()))
        })
        .unwrap()
        .unwrap();
        assert_eq!(
            options.root,
            home.path()
                .join("Library/Application Support/ClipCrop Points")
        );
        assert!(!options.root.exists(), "Parsing must not create a profile");
    }

    #[test]
    fn macos_missing_or_invalid_home_does_not_fall_back_to_a_portable_profile() {
        for home in [None, Some("".into()), Some("relative-home".into())] {
            assert!(macos_data_dir(home).is_err());
        }
    }
}

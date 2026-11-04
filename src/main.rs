#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
mod app;
mod core;
mod crypto;
mod http;
mod media;
mod peer;
pub mod rating_distribution;
mod torrent;
mod update;
mod update_install;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::{fs::OpenOptions, path::PathBuf, time::Duration};

struct Options {
    root: PathBuf,
    p2p: u16,
    torrent: u16,
    http: u16,
    headless: bool,
}

#[cfg(target_os = "macos")]
fn macos_data_dir(home: Option<std::ffi::OsString>) -> Result<PathBuf> {
    let home = PathBuf::from(home.context("Cannot locate home folder; use --data-dir")?);
    anyhow::ensure!(home.is_absolute(), "Invalid home folder; use --data-dir");
    // Keep the original profile location across app renames.
    // Never inspect a sibling profile: even checking Downloads can trigger a prompt.
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

fn default_data_dir() -> Result<PathBuf> {
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
        let exe = std::env::current_exe()?;
        let parent = exe.parent().context("Cannot locate executable folder")?;
        Ok(parent.join("clipcrop-points-data"))
    }
}

fn parse_options(
    args: impl IntoIterator<Item = String>,
    default_root: impl FnOnce() -> Result<PathBuf>,
) -> Result<Option<Options>> {
    let mut root = None;
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
                    "clipclub — a peer-to-peer video commons\n\nOptions:\n  --data-dir PATH       Use a specific profile folder\n  --p2p-port PORT       Peer mesh TCP port (42931)\n  --torrent-port PORT   BitTorrent TCP port (42932)\n  --http-port PORT      Local interface port (random by default)\n  --headless            Serve the local interface without a desktop window"
                );
                return Ok(None);
            }
            "--data-dir" => {
                root = Some(PathBuf::from(
                    args.next().context("--data-dir needs a path")?,
                ))
            }
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
        None => default_root()?,
    };
    Ok(Some(options))
}

fn options() -> Result<Option<Options>> {
    let Some(mut options) = parse_options(std::env::args().skip(1), default_data_dir)? else {
        return Ok(None);
    };
    std::fs::create_dir_all(&options.root)
        .context("Cannot create data folder; use --data-dir with a writable location")?;
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
        .open(lock_path)?;
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
        "clipclub interface: {url}\nPeer identity: {}",
        app.mesh.peer_id
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
            || panic!("An explicit profile must not inspect the default location"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.root, PathBuf::from("chosen-profile"));
        assert!(options.headless);
    }

    #[test]
    fn help_does_not_resolve_a_profile_folder() {
        assert!(
            parse_options(["--help".to_owned()], || {
                panic!("Help must not require access to a profile folder")
            })
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn linux_profile_uses_absolute_xdg_data_home_without_requiring_home() {
        let data = tempfile::tempdir().unwrap();
        let options = parse_options([], || linux_data_dir(Some(data.path().into()), None))
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_default_profile_is_in_application_support() {
        let home = tempfile::tempdir().unwrap();
        let options = parse_options([], || macos_data_dir(Some(home.path().into())))
            .unwrap()
            .unwrap();
        assert_eq!(
            options.root,
            home.path()
                .join("Library/Application Support/ClipCrop Points")
        );
        assert!(!options.root.exists(), "Parsing must not create a profile");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_missing_or_invalid_home_does_not_fall_back_to_a_portable_profile() {
        for home in [None, Some("".into()), Some("relative-home".into())] {
            assert!(macos_data_dir(home).is_err());
        }
    }
}

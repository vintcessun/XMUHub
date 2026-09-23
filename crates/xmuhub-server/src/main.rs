mod alloc;
mod api;
mod config;
mod web;

use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use xmuhub_core::db::Db;
use xmuhub_core::hub::{Hub, Limits};
use xmuhub_core::model::Level;
use xmuhub_core::storage::github::{GitHubBackend, GitHubConfig};
use xmuhub_core::storage::local::LocalBackend;
use xmuhub_core::storage::mirrors::Mirrors;
use xmuhub_core::storage::{Storage, StorageBackend};

use crate::config::{Config, StorageKind};

#[derive(Parser)]
#[command(name = "xmuhub", about = "XMUHub — 厦门大学学生资料共享平台")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the web server (default).
    Serve,
    /// Issue an access token and print its secret once.
    Token {
        /// 1 贡献者, 2 可信贡献者, 3 审核员, 4 管理员
        #[arg(long)]
        level: u8,
        #[arg(long, default_value = "")]
        label: String,
    },
    /// Write a JSON dump of the database to stdout.
    Export,
}

fn main() -> anyhow::Result<()> {
    let (mi_version, purge_before, purge_after) = alloc::tune();
    tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tantivy=warn".into()),
    )
    .init();
    tracing::debug!(mi_version, purge_before, purge_after, "mimalloc tuned");

    let cli = Cli::parse();
    let cfg = Config::from_env()?;
    // Two cores on the host: keep the runtime small and predictable.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(8)
        .enable_all()
        .build()?;
    rt.block_on(run(cli.cmd.unwrap_or(Cmd::Serve), cfg))
}

async fn run(cmd: Cmd, cfg: Config) -> anyhow::Result<()> {
    let db = Arc::new(Db::open(&cfg.data_dir.join("xmuhub.redb"), cfg.db_cache_mb * 1024 * 1024)?);
    let mirrors = Arc::new(Mirrors::new(cfg.mirrors.clone()));

    let mut http = reqwest::Client::builder()
        .user_agent("XMUHub/0.1 (+https://xmu.vintces.icu)")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60));
    if let Some(p) = &cfg.gh_api_proxy {
        http = http.proxy(reqwest::Proxy::all(p)?);
    }
    let http = http.build()?;
    // Mirror probes must see what users in China see, so they never use the proxy.
    let probe_http = reqwest::Client::builder().user_agent("XMUHub-probe/0.1").build()?;

    let local = Arc::new(LocalBackend { dir: cfg.data_dir.join("files"), secret: cfg.ticket_secret.clone() });
    let (storage, github, local_opt) = match cfg.storage {
        StorageKind::GitHub => {
            let gh = Arc::new(GitHubBackend::new(
                GitHubConfig {
                    owner: cfg.gh_owner.clone(),
                    token: cfg.gh_token.clone(),
                    repo_prefix: cfg.gh_repo_prefix.clone(),
                    assets_per_release: 900,
                    releases_per_repo: 50,
                    worker_url: cfg.worker_url.clone(),
                    ticket_secret: cfg.ticket_secret.clone(),
                    backup_repo: cfg.gh_backup_repo.clone(),
                },
                http.clone(),
                db.clone(),
                mirrors.clone(),
            ));
            // Local stays registered so blobs stored during development remain readable.
            let backends: Vec<Arc<dyn StorageBackend>> = vec![gh.clone(), local.clone()];
            (Storage::new(backends), Some(gh), None)
        }
        StorageKind::Local => (Storage::new(vec![local.clone() as Arc<dyn StorageBackend>]), None, Some(local.clone())),
    };
    let hub = Arc::new(Hub::open(db, storage, Limits::default())?);

    match cmd {
        Cmd::Token { level, label } => {
            let level = Level::from_u8(level).filter(|l| *l > Level::Guest).ok_or_else(|| anyhow::anyhow!("level must be 1-4"))?;
            let (secret, t) = hub.issue_token(level, &label, "cli")?;
            println!("level {} token (id {}):\n{secret}", t.level as u8, t.id);
            return Ok(());
        }
        Cmd::Export => {
            use std::io::Write;
            std::io::stdout().write_all(&hub.export()?)?;
            return Ok(());
        }
        Cmd::Serve => {}
    }

    let site = Arc::new(web::Site::load(&cfg.web_dir)?);
    let app = Arc::new(api::App {
        hub: hub.clone(),
        site,
        mirrors: mirrors.clone(),
        local: local_opt,
        worker_url: cfg.worker_url.clone(),
        claims: Default::default(),
    });
    spawn_jobs(app.clone(), github, probe_http);

    let router = api::router(app.clone()).layer(tower_http::trace::TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    tracing::info!("listening on http://{}", cfg.bind);
    axum::serve(listener, router).with_graceful_shutdown(shutdown()).await?;
    hub.flush_downloads()?;
    tracing::info!("bye");
    Ok(())
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("sigterm handler");
        tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}

fn spawn_jobs(app: Arc<api::App>, github: Option<Arc<GitHubBackend>>, probe_http: reqwest::Client) {
    // Download counters + returning memory to the OS.
    let hub = app.hub.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let h = hub.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || h.flush_downloads()).await.unwrap_or(Ok(())) {
                tracing::error!("flush downloads: {e}");
            }
            alloc::collect();
        }
    });

    // Abandoned uploads.
    let app2 = app.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let h = app2.hub.clone();
            match tokio::task::spawn_blocking(move || h.collect_garbage()).await {
                Ok(Ok(garbage)) => api::delete_later(&app2, garbage),
                Ok(Err(e)) => tracing::error!("gc: {e}"),
                Err(e) => tracing::error!("gc task: {e}"),
            }
        }
    });

    let Some(gh) = github else { return };

    // Mirror health: probe a tiny asset through every mirror, re-rank.
    let mirrors = app.mirrors.clone();
    let gh2 = gh.clone();
    tokio::spawn(async move {
        let probe = loop {
            match gh2.ensure_probe().await {
                Ok(url) => break url,
                Err(e) => {
                    tracing::warn!("probe asset: {e}; retrying in 5 min");
                    tokio::time::sleep(Duration::from_secs(300)).await;
                }
            }
        };
        let mut tick = tokio::time::interval(Duration::from_secs(20 * 60));
        loop {
            tick.tick().await;
            mirrors.probe(&probe_http, &probe).await;
        }
    });

    // Daily off-site metadata backup to a private repo.
    let hub = app.hub.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(24 * 3600));
        loop {
            tick.tick().await;
            let h = hub.clone();
            let dump = match tokio::task::spawn_blocking(move || h.export()).await {
                Ok(Ok(d)) => d,
                other => {
                    tracing::error!("backup export failed: {:?}", other.err());
                    continue;
                }
            };
            let gz = {
                use std::io::Write;
                let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
                let _ = e.write_all(&dump);
                e.finish().unwrap_or_default()
            };
            let day = xmuhub_core::model::now() / 86400;
            match gh.backup(&format!("xmuhub-{day}.json.gz"), gz, 30).await {
                Ok(()) => tracing::info!("metadata backup uploaded"),
                Err(e) => tracing::error!("metadata backup: {e}"),
            }
        }
    });
}

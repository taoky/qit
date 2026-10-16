use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use qit_core::{Cancel, Progress, catalog, install, network::Network, settings};
use std::{path::PathBuf, sync::Arc};

#[derive(Parser)]
#[command(version, about = "Qit — A GTK installer for Qt. Automation interface.")]
struct Args {
    #[arg(long, global = true)]
    mirror: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Mirrors,
    Repositories {
        #[arg(long, default_value_t = catalog::host().to_string())]
        host: String,
        #[arg(long, default_value = "desktop")]
        target: String,
    },
    Packages {
        #[arg(long, required = true)]
        repo: Vec<String>,
    },
    Plan {
        #[arg(long, required = true)]
        repo: Vec<String>,
        #[arg(required = true)]
        packages: Vec<String>,
    },
    Install {
        #[arg(long, required = true)]
        repo: Vec<String>,
        #[arg(required = true)]
        packages: Vec<String>,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        name: String,
        #[arg(long)]
        accept_licenses: bool,
        /// Add components to an existing Qit-managed installation.
        #[arg(long)]
        add_components: bool,
    },
    List,
    Recover {
        #[arg(long)]
        root: Option<PathBuf>,
    },
    Uninstall {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let progress: Progress = if args.json {
        Arc::new(|e| eprintln!("{}", serde_json::to_string(&e).unwrap()))
    } else {
        Arc::new(|e| {
            if e.phase != "download" {
                eprintln!("[{}] {}", e.phase, e.message)
            }
        })
    };
    let cancel = Cancel::default();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancel.cancel();
        }
    });
    let result = match args.command {
        Command::Mirrors => serde_json::to_value(settings::mirrors())?,
        Command::List => serde_json::to_value(install::list()?)?,
        Command::Recover { root } => serde_json::to_value(install::recover(
            &root.unwrap_or_else(settings::default_root),
        )?)?,
        Command::Uninstall { id, yes } => {
            ensure!(yes, "Pass --yes to uninstall this Qit-managed installation");
            serde_json::to_value(install::uninstall(&id, &cancel, &progress)?)?
        }
        command => {
            let mirror = args.mirror.or(settings::Settings::load()?.mirror).context("Select a mirror with --mirror URL. See `qit-cli mirrors` for the bundled Qt mirror list.")?;
            let net = Network::new(&mirror, cancel.clone(), progress.clone())?;
            match command {
                Command::Repositories { host, target } => {
                    serde_json::to_value(catalog::repositories(&net, &host, &target).await?)?
                }
                Command::Packages { repo } => serde_json::to_value(load(&net, &repo).await?)?,
                Command::Plan { repo, packages } => serde_json::to_value(
                    catalog::prepare_plan(&net, load(&net, &repo).await?, &packages).await?,
                )?,
                Command::Install {
                    repo,
                    packages,
                    root,
                    name,
                    accept_licenses,
                    add_components,
                } => {
                    ensure!(
                        accept_licenses,
                        "Review component licenses at https://www.qt.io/licensing/ and pass --accept-licenses to confirm you accept the applicable licenses"
                    );
                    let root = root.unwrap_or_else(settings::default_root);
                    let existing = if add_components {
                        Some(
                            install::at_path(&root.join(&name))?
                                .context("No existing Qit installation to extend")?,
                        )
                    } else {
                        None
                    };
                    let mut all = load(&net, &repo).await?;
                    if let Some(old) = &existing {
                        all.retain(|p| !old.plan.packages.iter().any(|o| o.id == p.id));
                        all.extend(old.plan.packages.clone());
                    }
                    let plan = catalog::prepare_plan(&net, all, &packages).await?;
                    serde_json::to_value(if let Some(old) = existing {
                        install::extend(plan, &old, cancel, progress).await?
                    } else {
                        install::install(plan, &root, &name, cancel, progress).await?
                    })?
                }
                _ => unreachable!(),
            }
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
async fn load(net: &Network, repositories: &[String]) -> Result<Vec<catalog::Package>> {
    let mut packages = Vec::new();
    for repo in repositories {
        packages.extend(catalog::packages(net, repo).await?);
    }
    Ok(packages)
}

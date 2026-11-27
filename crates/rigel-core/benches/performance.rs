use std::collections::BTreeMap;
use std::future::Future;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use rigel_core::patches::{
    PatchPipeline, StopAt, apply_patch_text, parse_diff_hunks, unified_diff,
};
use rigel_core::search::{SearchQuery, search_history, workspace};
use rigel_core::{SessionService, index::hash::tree_hash};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

const DEFAULT_SAMPLES: usize = 15;
const REGRESSION_LIMIT_PERCENT: u32 = 25;
const ITERATIONS_PER_SAMPLE: usize = 10;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct Baseline {
    median_ns: u128,
    regression_limit_percent: u32,
}

#[derive(Debug, Deserialize, Serialize)]
struct BaselineFile {
    version: u32,
    benchmarks: BTreeMap<String, Baseline>,
}

type Measurements = BTreeMap<String, u128>;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("performance bench: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--check-policy") {
        assert!(!is_regression(125, 100, REGRESSION_LIMIT_PERCENT));
        assert!(is_regression(126, 100, REGRESSION_LIMIT_PERCENT));
        println!("baseline threshold policy: PASS at limit, FAIL beyond limit");
        return Ok(());
    }
    let record = args.iter().any(|arg| arg == "--record-baseline");
    let samples = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--samples="))
        .map(str::parse)
        .transpose()?
        .unwrap_or(DEFAULT_SAMPLES);
    if !(3..=500).contains(&samples) {
        return Err("--samples must be between 3 and 500".into());
    }

    let measurements = measure_all(samples).await?;
    let baseline_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/baseline.json");
    if record {
        let file = BaselineFile {
            version: 1,
            benchmarks: measurements
                .into_iter()
                .map(|(name, median_ns)| {
                    (
                        name,
                        Baseline {
                            median_ns,
                            regression_limit_percent: REGRESSION_LIMIT_PERCENT,
                        },
                    )
                })
                .collect(),
        };
        std::fs::write(&baseline_path, serde_json::to_vec_pretty(&file)?)?;
        println!(
            "recorded {} baselines at {}",
            file.benchmarks.len(),
            baseline_path.display()
        );
        return Ok(());
    }

    let baseline: BaselineFile = serde_json::from_slice(&std::fs::read(&baseline_path)?)?;
    if baseline.version != 1 {
        return Err(format!("unsupported baseline version {}", baseline.version).into());
    }
    if baseline.benchmarks.len() != measurements.len() {
        return Err(
            "baseline case set differs from this benchmark suite; record a new baseline".into(),
        );
    }

    let mut failed = false;
    for (name, median_ns) in measurements {
        let Some(previous) = baseline.benchmarks.get(&name) else {
            return Err(
                format!("baseline is missing benchmark {name}; record a new baseline").into(),
            );
        };
        let delta = percent_delta(median_ns, previous.median_ns);
        let regression = is_regression(
            median_ns,
            previous.median_ns,
            previous.regression_limit_percent,
        );
        println!(
            "{name}: median={median_ns}ns baseline={}ns trend={delta:+.1}% threshold=+{}% {}",
            previous.median_ns,
            previous.regression_limit_percent,
            if regression { "REGRESSION" } else { "ok" },
        );
        failed |= regression;
    }
    if failed {
        return Err("one or more benchmarks exceeded their recorded regression threshold".into());
    }
    Ok(())
}

fn is_regression(current: u128, baseline: u128, limit_percent: u32) -> bool {
    current.saturating_mul(100) > baseline.saturating_mul(u128::from(100 + limit_percent))
}

fn percent_delta(current: u128, baseline: u128) -> f64 {
    if baseline == 0 {
        return if current == 0 { 0.0 } else { f64::INFINITY };
    }
    (current as f64 / baseline as f64 - 1.0) * 100.0
}

fn measure_sync(mut run: impl FnMut(), samples: usize) -> u128 {
    for _ in 0..2 {
        for _ in 0..ITERATIONS_PER_SAMPLE {
            run();
        }
    }
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..ITERATIONS_PER_SAMPLE {
            run();
        }
        timings.push(start.elapsed().as_nanos() / ITERATIONS_PER_SAMPLE as u128);
    }
    median(timings)
}

async fn measure_async<F, Fut>(mut run: F, samples: usize) -> u128
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    for _ in 0..2 {
        for _ in 0..ITERATIONS_PER_SAMPLE {
            run().await;
        }
    }
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..ITERATIONS_PER_SAMPLE {
            run().await;
        }
        timings.push(start.elapsed().as_nanos() / ITERATIONS_PER_SAMPLE as u128);
    }
    median(timings)
}

fn median(mut values: Vec<u128>) -> u128 {
    values.sort_unstable();
    values[values.len() / 2]
}

async fn measure_all(samples: usize) -> Result<Measurements, Box<dyn std::error::Error>> {
    let mut measurements = BTreeMap::new();
    let tree = TempDir::new()?;
    for directory in 0..8 {
        let path = tree.path().join(format!("dir-{directory}"));
        std::fs::create_dir_all(&path)?;
        for file in 0..32 {
            std::fs::write(
                path.join(format!("file-{file}.rs")),
                format!("// fixture {directory}:{file}\nfn value() -> usize {{ {file} }}\n"),
            )?;
        }
    }
    let tree_root = tree.path().to_path_buf();
    measurements.insert(
        "index.tree_hash".into(),
        measure_sync(
            || {
                black_box(tree_hash(&tree_root).expect("tree hash"));
            },
            samples,
        ),
    );

    let history = TempDir::new()?;
    let service = SessionService::open(history.path()).await?;
    for session_index in 0..12 {
        let session = service
            .start(Some(&format!("search fixture {session_index}")), None)
            .await?;
        let agent = service
            .register_agent(&session.id, "native", Some("bench-agent"))
            .await?;
        for turn_index in 0..3 {
            let turn = service
                .append_turn(
                    &agent.id,
                    Some(&format!("needle prompt {session_index} {turn_index}")),
                )
                .await?;
            service.append_item(
                &turn.id,
                "message",
                "completed",
                &serde_json::json!({"text": format!("needle answer {session_index} {turn_index}")}),
                &serde_json::json!({"type": "message"}),
            ).await?;
            service.append_item(
                &turn.id,
                "command",
                "completed",
                &serde_json::json!({"command": format!("cargo test needle-{session_index}-{turn_index}")}),
                &serde_json::json!({"type": "command"}),
            ).await?;
        }
    }
    measurements.insert(
        "search.workspace".into(),
        measure_async(
            || async {
                black_box(
                    workspace(service.pool(), "needle", None, 25)
                        .await
                        .expect("workspace search"),
                );
            },
            samples,
        )
        .await,
    );
    let query = SearchQuery {
        text: "needle".into(),
        ..SearchQuery::default()
    };
    measurements.insert(
        "search.query".into(),
        measure_async(
            || async {
                black_box(
                    search_history(service.pool(), &query)
                        .await
                        .expect("history search"),
                );
            },
            samples,
        )
        .await,
    );

    let old = (0..1200)
        .map(|line| format!("line {line}: original value\n"))
        .collect::<String>();
    let new = (0..1200)
        .map(|line| {
            if (590..610).contains(&line) {
                format!("line {line}: revised value\n")
            } else {
                format!("line {line}: original value\n")
            }
        })
        .collect::<String>();
    let diff = unified_diff("src/fixture.rs", &old, &new);
    measurements.insert(
        "patch.unified_diff".into(),
        measure_sync(
            || {
                black_box(unified_diff("src/fixture.rs", &old, &new));
            },
            samples,
        ),
    );
    measurements.insert(
        "patch.parse_diff_hunks".into(),
        measure_sync(
            || {
                black_box(parse_diff_hunks(&diff));
            },
            samples,
        ),
    );
    measurements.insert(
        "patch.apply_patch_text".into(),
        measure_sync(
            || {
                black_box(apply_patch_text(&diff, &old).expect("apply text patch"));
            },
            samples,
        ),
    );

    let project = TempDir::new()?;
    let patch_service = SessionService::open(project.path()).await?;
    let session = patch_service.start(Some("patch bench"), None).await?;
    let agent = patch_service
        .register_agent(&session.id, "native", Some("bench-agent"))
        .await?;
    let target = project.path().join("fixture.txt");
    std::fs::write(&target, "initial content\n")?;
    let pipeline = PatchPipeline::new(project.path(), patch_service.pool().clone());
    let mut revision = 0usize;
    measurements.insert(
        "patch.propose".into(),
        measure_async(
            || {
                revision += 1;
                let content = format!("content revision {revision}\n");
                let hash = pipeline.read("fixture.txt").expect("read base").1;
                let pipeline = &pipeline;
                let session_id = session.id.clone();
                let agent_id = agent.id.clone();
                async move {
                    black_box(
                        pipeline
                            .propose(&session_id, &agent_id, "fixture.txt", &hash, &content)
                            .await
                            .expect("propose"),
                    );
                }
            },
            samples,
        )
        .await,
    );

    let mut apply_timings = Vec::with_capacity(samples);
    for iteration in 0..(samples + 3) {
        let (base, base_hash) = pipeline.read("fixture.txt")?;
        let proposed = format!("apply revision {iteration}\n");
        let pending = pipeline
            .propose_with(
                &session.id,
                &agent.id,
                "fixture.txt",
                &base_hash,
                &proposed,
                StopAt::AfterJournal,
            )
            .await;
        if !matches!(
            pending,
            Err(rigel_core::patches::PatchError::Stopped(
                StopAt::AfterJournal
            ))
        ) {
            return Err("failed to prepare proposed patch for apply benchmark".into());
        }
        let patch_id: String = sqlx::query_scalar("SELECT id FROM patches WHERE path = 'fixture.txt' ORDER BY created_at DESC, rowid DESC LIMIT 1")
            .fetch_one(patch_service.pool()).await?;
        let start = Instant::now();
        black_box(pipeline.apply(&patch_id).await?);
        let elapsed = start.elapsed().as_nanos();
        if iteration >= 3 {
            apply_timings.push(elapsed);
        }
        black_box(base);
    }
    measurements.insert("patch.apply".into(), median(apply_timings));
    Ok(measurements)
}

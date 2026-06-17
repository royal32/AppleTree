use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use macdirstat::bench;
use macdirstat::settings::{AppPrefs, TableColumn};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            eprintln!();
            eprintln!("{}", usage());
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let config = Config::parse(std::env::args().skip(1).collect())?;
    match config.command {
        Command::Scan => run_scan(&config),
        Command::TableSort => run_table_sort(&config),
        Command::TreemapRender => run_treemap_render(&config),
    }
}

fn run_scan(config: &Config) -> Result<(), String> {
    for _ in 0..config.warmups {
        let _ = bench::scan(&config.path);
    }

    let mut samples = Vec::with_capacity(config.runs);
    for run in 1..=config.runs {
        let (_, result) = bench::scan(&config.path);
        println!(
            "scan\trun={run}\ttime={}\tnodes={}\tbytes={}\textensions={}",
            secs(result.duration),
            result.nodes,
            result.bytes,
            result.extensions
        );
        samples.push(result.duration);
    }
    print_summary("scan", &samples);
    Ok(())
}

fn run_table_sort(config: &Config) -> Result<(), String> {
    let (tree, scan) = bench::scan(&config.path);
    println!(
        "load\ttime={}\tnodes={}\tbytes={}\textensions={}",
        secs(scan.duration),
        scan.nodes,
        scan.bytes,
        scan.extensions
    );

    let prefs = AppPrefs {
        sort_column: config.sort_column,
        sort_descending: config.sort_descending,
        ..Default::default()
    };

    for _ in 0..config.warmups {
        let _ = bench::table_sort(&tree, &prefs);
    }

    let mut samples = Vec::with_capacity(config.runs);
    for run in 1..=config.runs {
        let result = bench::table_sort(&tree, &prefs);
        println!(
            "table-sort\trun={run}\ttime={}\tdirectories={}\tchildren={}",
            secs(result.duration),
            result.directories,
            result.sorted_children
        );
        samples.push(result.duration);
    }
    print_summary("table-sort", &samples);
    Ok(())
}

fn run_treemap_render(config: &Config) -> Result<(), String> {
    let (tree, scan) = bench::scan(&config.path);
    println!(
        "load\ttime={}\tnodes={}\tbytes={}\textensions={}",
        secs(scan.duration),
        scan.nodes,
        scan.bytes,
        scan.extensions
    );

    let prefs = AppPrefs::default();
    for _ in 0..config.warmups {
        let _ = bench::treemap_render(&tree, &prefs, config.width, config.height);
    }

    let mut total_samples = Vec::with_capacity(config.runs);
    let mut layout_samples = Vec::with_capacity(config.runs);
    let mut render_samples = Vec::with_capacity(config.runs);
    for run in 1..=config.runs {
        let result = bench::treemap_render(&tree, &prefs, config.width, config.height);
        println!(
            "treemap-render\trun={run}\ttotal={}\tlayout={}\trender={}\tleaves={}\tpixels={}",
            secs(result.total),
            secs(result.layout),
            secs(result.render),
            result.leaves,
            result.pixels
        );
        total_samples.push(result.total);
        layout_samples.push(result.layout);
        render_samples.push(result.render);
    }
    print_summary("treemap-total", &total_samples);
    print_summary("treemap-layout", &layout_samples);
    print_summary("treemap-render", &render_samples);
    Ok(())
}

#[derive(Clone, Copy)]
enum Command {
    Scan,
    TableSort,
    TreemapRender,
}

struct Config {
    command: Command,
    path: PathBuf,
    runs: usize,
    warmups: usize,
    width: usize,
    height: usize,
    sort_column: TableColumn,
    sort_descending: bool,
}

impl Config {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        let mut iter = args.into_iter();
        let command = match iter.next().as_deref() {
            Some("scan") => Command::Scan,
            Some("table-sort") => Command::TableSort,
            Some("treemap-render") => Command::TreemapRender,
            Some("-h") | Some("--help") => return Err(String::new()),
            Some(other) => return Err(format!("unknown command: {other}")),
            None => Command::Scan,
        };

        let mut path = None;
        let mut runs = 5usize;
        let mut warmups = 1usize;
        let mut width = 1200usize;
        let mut height = 800usize;
        let mut sort_column = TableColumn::Name;
        let mut sort_descending = false;

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--runs" => runs = parse_next(&mut iter, "--runs")?,
                "--warmups" => warmups = parse_next(&mut iter, "--warmups")?,
                "--width" => width = parse_next(&mut iter, "--width")?,
                "--height" => height = parse_next(&mut iter, "--height")?,
                "--sort" => sort_column = parse_sort_column(&next_arg(&mut iter, "--sort")?)?,
                "--asc" => sort_descending = false,
                "--desc" => sort_descending = true,
                "-h" | "--help" => return Err(String::new()),
                _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
                _ if path.is_none() => path = Some(PathBuf::from(arg)),
                _ => return Err(format!("unexpected extra path/argument: {arg}")),
            }
        }

        if runs == 0 {
            return Err("--runs must be greater than zero".to_owned());
        }

        let path = path
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .ok_or_else(|| "path or HOME required".to_owned())?;

        Ok(Self {
            command,
            path,
            runs,
            warmups,
            width,
            height,
            sort_column,
            sort_descending,
        })
    }
}

fn next_arg(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_next<T>(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    next_arg(iter, flag)?
        .parse()
        .map_err(|_| format!("invalid value for {flag}"))
}

fn parse_sort_column(value: &str) -> Result<TableColumn, String> {
    match value {
        "name" => Ok(TableColumn::Name),
        "size" => Ok(TableColumn::Size),
        "parent-pct" => Ok(TableColumn::PercentOfParent),
        "items" => Ok(TableColumn::Items),
        "files" => Ok(TableColumn::Files),
        "folders" => Ok(TableColumn::Folders),
        "modified" => Ok(TableColumn::Modified),
        _ => Err(format!("unknown sort column: {value}")),
    }
}

fn print_summary(label: &str, samples: &[Duration]) {
    let stats = Stats::from(samples);
    println!(
        "summary\t{name}\truns={}\tmin={}\tmedian={}\tmean={}\tmax={}",
        samples.len(),
        secs(stats.min),
        secs(stats.median),
        secs(stats.mean),
        secs(stats.max),
        name = label
    );
}

struct Stats {
    min: Duration,
    median: Duration,
    mean: Duration,
    max: Duration,
}

impl Stats {
    fn from(samples: &[Duration]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let total = samples.iter().map(Duration::as_nanos).sum::<u128>();
        let mean = Duration::from_nanos((total / samples.len() as u128) as u64);
        Self {
            min: sorted[0],
            median: sorted[sorted.len() / 2],
            mean,
            max: sorted[sorted.len() - 1],
        }
    }
}

fn secs(duration: Duration) -> String {
    format!("{:.6}s", duration.as_secs_f64())
}

fn usage() -> &'static str {
    "usage:
  cargo run --release --bin bench -- scan [path] [--runs N] [--warmups N]
  cargo run --release --bin bench -- table-sort [path] [--runs N] [--warmups N] [--sort name|size|parent-pct|items|files|folders|modified] [--asc|--desc]
  cargo run --release --bin bench -- treemap-render [path] [--runs N] [--warmups N] [--width PX] [--height PX]"
}

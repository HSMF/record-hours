use std::{
    collections::BTreeMap,
    fmt::Display,
    fs::File,
    io::{Read, Write},
    iter::Sum,
    path::PathBuf,
    str::FromStr,
};

use anyhow::{anyhow, Context};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
struct Date(NaiveDate);

impl FromStr for Date {
    type Err = chrono::ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let inner = NaiveDate::parse_from_str(s, "%Y-%m-%d")?;
        Ok(Date(inner))
    }
}

impl Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
struct Time(NaiveTime);

impl Display for Time {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.format("%H:%M"))?;
        Ok(())
    }
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(transparent)]
pub struct Log {
    projects: BTreeMap<String, Project>,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(transparent)]
pub struct Project {
    entries: BTreeMap<Date, Vec<TimeStamp>>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct Entry {
    date: Date,
    timestamps: Vec<TimeStamp>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct TimeStamp {
    #[serde(rename = "type")]
    typ: TimeStampType,
    time: Time,
    /// tolerance (for how to merge entries) in seconds
    tolerance: u32,
}

impl TimeStamp {
    fn is_start(&self) -> bool {
        self.typ == TimeStampType::Start
    }

    fn is_end(&self) -> bool {
        self.typ == TimeStampType::End
    }
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimeStampType {
    Start,
    End,
}

#[derive(Debug, Clone, Parser)]
pub struct App {
    /// which file to record the hours in
    #[clap(short, long)]
    file: Option<PathBuf>,
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    Record {
        #[clap(short, long)]
        auto: bool,
        #[clap(short, long)]
        project: Option<String>,
    },
    Show {
        #[clap(short, long)]
        project: Option<String>,
        /// display time in decimal format: e.g. 1 hour, 45 minutes = 1.75
        #[clap(short, long)]
        decimal: bool,
    },
}

struct Record {
    log: Log,
    date: Date,
    time: Time,
}

impl Record {
    fn open(mut input: impl Read) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        input.read_to_end(&mut buf)?;

        let log: Log = if buf.is_empty() {
            log::warn!("file was empty, using default");
            Log::default()
        } else {
            serde_json::from_slice(&buf)?
        };

        log::info!("read {log:#?}");

        let now = chrono::offset::Local::now();
        let date = now.date_naive();
        let time = now.time();
        Ok(Self {
            log,
            date: Date(date),
            time: Time(time),
        })
    }

    fn insert(&mut self, project: String) {
        let entry = self
            .log
            .projects
            .entry(project)
            .or_default()
            .entries
            .entry(self.date)
            .or_default();

        if let Some(last_timestamp) = entry.last_mut() {
            let dur = Duration::seconds(last_timestamp.tolerance as i64);
            let now = NaiveDateTime::new(self.date.0, self.time.0);
            let last_acceptable = NaiveDateTime::new(self.date.0, last_timestamp.time.0) + dur;
            if last_timestamp.is_end() && now <= last_acceptable {
                last_timestamp.time = self.time;
                return;
            }
        }

        let typ = if entry.last().is_some_and(|l| l.is_start()) {
            TimeStampType::End
        } else {
            TimeStampType::Start
        };

        entry.push(TimeStamp {
            typ,
            time: self.time,
            tolerance: 60 * 15,
        });
    }

    fn commit(&self, output: impl Write) -> anyhow::Result<()> {
        serde_json::to_writer_pretty(output, &self.log)?;
        Ok(())
    }
}

struct Item {
    start: Time,
    end: Time,
}

impl Item {
    fn duration(&self) -> i64 {
        let delta = self.end.0 - self.start.0;
        delta.num_seconds()
    }
}

struct MyDuration(Duration);

impl Display for MyDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.num_hours() != 0 {
            write!(f, "{}h", self.0.num_hours())?;
        }
        write!(f, "{}min", self.0.num_minutes() % 60)?;
        Ok(())
    }
}

struct DecimalDuration(Duration);

impl Display for DecimalDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.02}", self.0.num_minutes() as f64 / 60.0)?;
        Ok(())
    }
}

fn show(input: impl Read, project: &str, decimal: bool) -> anyhow::Result<()> {
    fn get_times<'a>(
        mut iter: impl Iterator<Item = &'a TimeStamp>,
        mut start: Time,
    ) -> (Vec<Item>, Option<Time>) {
        let mut items = vec![];
        while let Some(head) = iter.next() {
            if head.is_start() {
                start = head.time;
            } else {
                items.push(Item {
                    start,
                    end: head.time,
                });
                let Some(next) = iter.find(|x| x.is_start()) else {
                    return (items, None);
                };
                start = next.time;
            }
        }
        (items, Some(start))
    }

    let stored: Log = serde_json::from_reader(input).context("input file was missing")?;

    let project_info = stored
        .projects
        .get(project)
        .ok_or(anyhow!("project {project} is not present in log file"))?;

    for (date, day) in project_info.entries.iter() {
        let mut iter = day.iter();
        let Some(start) = iter.find(|x| x.is_start()) else {
            log::warn!("day {date} is present in {project} but was empty");
            continue;
        };
        let times = get_times(iter, start.time);

        {
            let mut f = std::io::stdout().lock();
            let duration: Duration = times.0.iter().map(|x| x.end.0 - x.start.0).sum();

            let duration: Box<dyn Display> = if decimal {
                Box::new(DecimalDuration(duration))
            } else {
                Box::new(MyDuration(duration))
            };
            writeln!(f, "{date} ({}):", duration)?;
            for Item { start, end } in times.0 {
                writeln!(f, "  - {start} - {end}")?;
            }
            if let Some(start) = times.1 {
                writeln!(f, "  - {start} - ")?;
            }
        }
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let app = App::parse();
    env_logger::init();

    match app.command {
        Commands::Record { auto: _, project } => {
            let project = project.unwrap_or_default();
            let path = app.file.unwrap_or_else(|| PathBuf::from("hours.log.json"));

            let mut recorder = if !path.exists() {
                Record::open(std::io::empty())?
            } else {
                let infile = File::open(&path)?;
                Record::open(infile)?
            };

            recorder.insert(project);

            let outfile = File::create(&path)?;

            // recorder.commit(std::io::stdout().lock())?;
            recorder.commit(outfile)?;
        }
        Commands::Show { project, decimal } => {
            let project = project.unwrap_or_default();
            let path = app.file.unwrap_or_else(|| PathBuf::from("hours.log.json"));
            let infile = File::open(path)?;
            show(infile, &project, decimal)?;
        }
    }

    Ok(())
}

use std::{
    collections::BTreeMap,
    fmt::Display,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    str::FromStr,
};

use anyhow::{Context, anyhow};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use clap::{Parser, Subcommand};
use log::warn;
use serde::{Deserialize, Serialize};

mod format;

const DEFAULT_TOLERANCE: u32 = 15 * 60;

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

#[derive(Deserialize, Serialize, Debug, Default, PartialEq, Eq)]
#[serde(transparent)]
pub struct Log {
    projects: BTreeMap<String, Project>,
}

#[derive(Deserialize, Serialize, Debug, Default, PartialEq, Eq)]
#[serde(transparent)]
pub struct Project {
    entries: BTreeMap<Date, Vec<TimeStamp>>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct Entry {
    date: Date,
    timestamps: Vec<TimeStamp>,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct TimeStamp {
    #[serde(rename = "type")]
    typ: TimeStampType,
    time: Time,
    /// tolerance (for how to merge entries) in seconds
    tolerance: u32,
    sub_project: Option<String>,
}

impl TimeStamp {
    fn is_start(&self) -> bool {
        self.typ == TimeStampType::Start
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
        #[clap(short, long)]
        sub_project: Option<String>,
    },
    Show {
        /// show for specific project.
        ///
        /// May be repeated.
        ///
        /// --project "" shows only the default project
        #[clap(short, long)]
        project: Vec<String>,
        /// display time in decimal format: e.g. 1 hour, 45 minutes = 1.75
        #[clap(short, long)]
        decimal: bool,
        /// Display format.
        ///
        /// Variables that can be used for expansion:
        /// %d => date in y-m-d
        /// %Y => year
        /// %M => month
        /// %D => day
        /// %t => decimal time that has been recorded
        /// %h => hours that have been recorded
        /// %m => minutes that have been recorded
        /// %P => the project
        /// %p => the subproject
        /// %% => a literal '%'
        #[clap(short, long)]
        format: Option<String>,
        /// show different sub projects
        #[clap(short, long)]
        group_by_subproject: bool,
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

    fn insert(&mut self, project: String, mut sub_project: Option<String>) {
        let entry = self
            .log
            .projects
            .entry(project)
            .or_default()
            .entries
            .entry(self.date)
            .or_default();

        let Some(last_timestamp) = entry.last() else {
            entry.push(TimeStamp {
                typ: TimeStampType::Start,
                time: self.time,
                tolerance: DEFAULT_TOLERANCE,
                sub_project,
            });
            return;
        };

        let same_project = last_timestamp.sub_project == sub_project;
        let is_start = last_timestamp.is_start();
        // same project, different project
        // last is Start, last is End
        // last is within tolerance, last is outside of tolerance
        //
        // s S y -> add end(s)
        // s S n -> add end(s)
        // s E y -> update time(s)
        // s E n -> add start(s)
        // d S y -> add end(s), add start(d)
        // d S n -> add end(s), add start(d)
        // d E y -> update time(s), add start(d)
        // d E n -> add start(d)
        if is_start {
            let sub_project = last_timestamp.sub_project.clone();
            entry.push(TimeStamp {
                typ: TimeStampType::End,
                time: self.time,
                tolerance: DEFAULT_TOLERANCE,
                sub_project,
            });
        } else {
            let dur = Duration::seconds(last_timestamp.tolerance as i64);
            let now = NaiveDateTime::new(self.date.0, self.time.0);
            let last_acceptable = NaiveDateTime::new(self.date.0, last_timestamp.time.0) + dur;
            if now <= last_acceptable {
                entry.last_mut().unwrap().time = self.time;
            } else if same_project {
                entry.push(TimeStamp {
                    typ: TimeStampType::Start,
                    time: self.time,
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: sub_project.take(),
                });
            }
        }

        if !same_project {
            entry.push(TimeStamp {
                typ: TimeStampType::Start,
                time: self.time,
                tolerance: DEFAULT_TOLERANCE,
                sub_project,
            });
        }
    }

    fn commit(&self, output: impl Write) -> anyhow::Result<()> {
        serde_json::to_writer_pretty(output, &self.log)?;
        Ok(())
    }
}

#[derive(Clone)]
struct Item {
    start: Time,
    end: Option<Time>,
    project: Option<String>,
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

fn show<F>(input: impl Read, project: &str, display: F) -> anyhow::Result<()>
where
    F: Fn(&Date, Vec<Item>) -> anyhow::Result<()>,
{
    fn get_times<'a>(iter: impl Iterator<Item = &'a TimeStamp>) -> Vec<Item> {
        let mut ret = vec![];
        let mut iter = iter.peekable();
        loop {
            let Some(head) = iter.next() else { break };
            if !head.is_start() {
                // bad input, must start with `start`, ignore
                continue;
            }

            let Some(tail) = iter.peek() else {
                ret.push(Item {
                    start: head.time,
                    end: None,
                    project: head.sub_project.clone(),
                });
                break;
            };
            if tail.is_start() {
                // two consecutive starts, ignore
                continue;
            }
            let tail = iter.next().unwrap();
            if tail.sub_project != head.sub_project {
                // not the same sub project, ignore
                continue;
            }
            ret.push(Item {
                start: head.time,
                end: Some(tail.time),
                project: head.sub_project.clone(),
            });
        }
        ret
    }

    let stored: Log = serde_json::from_reader(input).context("input file was missing")?;

    let project_info = stored
        .projects
        .get(project)
        .ok_or(anyhow!("project {project} is not present in log file"))?;

    for (date, day) in project_info.entries.iter() {
        let times = get_times(day.iter());
        if times.is_empty() {
            log::warn!("day {date} is present in {project} but was empty");
        }

        display(date, times)?;
    }

    Ok(())
}

/// coalesces times, ignoring the subproject
/// turns [09:00-10:00 (proj A), 10:00-11:00 (proj B)]
/// into [09:00-11:00 (proj A)]
fn coalesce_times(it: impl IntoIterator<Item = Item>) -> Vec<Item> {
    let mut it = it.into_iter();
    let Some(mut cur) = it.next() else {
        return vec![];
    };

    let mut ret = vec![];
    for i in it {
        if Some(i.start) == cur.end {
            cur.end = i.end;
        } else {
            ret.push(cur);
            cur = i;
        }
    }
    ret.push(cur);

    ret
}

fn main() -> anyhow::Result<()> {
    let app = App::parse();
    env_logger::init();

    match app.command {
        Commands::Record {
            auto: _,
            project,
            sub_project,
        } => {
            let project = project.unwrap_or_default();
            let path = app.file.unwrap_or_else(|| PathBuf::from("hours.log.json"));

            let mut recorder = if !path.exists() {
                Record::open(std::io::empty())?
            } else {
                let infile = File::open(&path)?;
                Record::open(infile)?
            };

            recorder.insert(project, sub_project);

            let outfile = File::create(&path)?;

            // recorder.commit(std::io::stdout().lock())?;
            recorder.commit(outfile)?;
        }
        Commands::Show {
            project,
            decimal,
            format,
            group_by_subproject,
        } => {
            if project.len() > 1 {
                warn!("specifying multiple projects isn't implemented atm")
            }
            let project = project.first().cloned().unwrap_or_default();
            let path = app.file.unwrap_or_else(|| PathBuf::from("hours.log.json"));
            let infile = File::open(path)?;

            // TODO: make use of group_by_subproject
            if group_by_subproject {
                todo!("grouping by subproject is not yet supported")
            }

            if let Some(format) = &format {
                show(infile, &project, |&date, times| {
                    let fmt = format::Formatter {
                        date,
                        times: &times,
                        format,
                        project: &project,
                    };
                    println!("{fmt}");
                    Ok(())
                })?;
                return Ok(());
            }
            show(infile, &project, |date, times| {
                let mut f = std::io::stdout().lock();
                let duration: Duration = times
                    .iter()
                    .filter_map(|x| x.end.map(|end| end.0 - x.start.0))
                    .sum();

                let duration = if decimal {
                    DecimalDuration(duration).to_string()
                } else {
                    MyDuration(duration).to_string()
                };
                writeln!(f, "{date} ({}):", duration)?;
                for Item {
                    start,
                    end,
                    project: _,
                } in coalesce_times(times)
                {
                    if let Some(end) = end {
                        writeln!(f, "  - {start} - {end}")?;
                    } else {
                        writeln!(f, "  - {start} - ")?;
                    }
                }
                Ok(())
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::zero_prefixed_literal)]
mod tests {
    use super::*;

    macro_rules! date {
        ($y:literal-$M:literal-$d:literal) => {
            Date(NaiveDate::from_ymd_opt($y, $M, $d).unwrap())
        };
    }
    macro_rules! time {
        ($h:literal:$m:literal:$s:literal) => {
            Time(NaiveTime::from_hms_opt($h, $m, $s).unwrap())
        };
    }
    macro_rules! record {
        ($y:literal-$M:literal-$d:literal, $h:literal:$m:literal:$s:literal, $log:expr) => {
            Record {
                log: $log,
                date: Date(NaiveDate::from_ymd_opt($y, $M, $d).unwrap()),
                time: Time(NaiveTime::from_hms_opt($h, $m, $s).unwrap()),
            }
        };
        ($y:literal-$M:literal-$d:literal, $h:literal:$m:literal:$s:literal) => {
            Record {
                log: Default::default(),
                date: Date(NaiveDate::from_ymd_opt($y, $M, $d).unwrap()),
                time: Time(NaiveTime::from_hms_opt($h, $m, $s).unwrap()),
            }
        };
    }

    fn with_empty_project(entries: BTreeMap<Date, Vec<TimeStamp>>) -> Log {
        Log {
            projects: BTreeMap::from([(String::new(), Project { entries })]),
        }
    }

    fn on_date(date: Date, timestamps: impl IntoIterator<Item = TimeStamp>) -> Log {
        with_empty_project(BTreeMap::from([(date, timestamps.into_iter().collect())]))
    }

    #[test]
    fn insert_at_empty_log() {
        let mut record = record!(2025-12-02, 11:00:00);
        record.insert(String::new(), None);
        assert_eq!(
            record.log,
            on_date(
                date!(2025 - 12 - 02),
                vec![TimeStamp {
                    typ: TimeStampType::Start,
                    time: time!(11:00:00),
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: None
                }]
            )
        );
    }

    #[test]
    fn insert_same_project_after_start() {
        let initial = on_date(
            date!(2025 - 12 - 02),
            vec![TimeStamp {
                typ: TimeStampType::Start,
                time: time!(11:00:00),
                tolerance: DEFAULT_TOLERANCE,
                sub_project: None,
            }],
        );
        let mut record = record!(2025-12-02, 11:03:00, initial);
        record.insert(String::new(), None);
        assert_eq!(
            record.log,
            on_date(
                date!(2025 - 12 - 02),
                [
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:00:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None
                    },
                    TimeStamp {
                        typ: TimeStampType::End,
                        time: time!(11:03:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None
                    }
                ],
            )
        );
    }

    #[test]
    fn insert_different_project_after_start() {
        let initial = on_date(
            date!(2025 - 12 - 02),
            vec![TimeStamp {
                typ: TimeStampType::Start,
                time: time!(11:00:00),
                tolerance: DEFAULT_TOLERANCE,
                sub_project: None,
            }],
        );
        let mut record = record!(2025-12-02, 11:03:00, initial);
        record.insert(String::new(), Some("foo".into()));
        assert_eq!(
            record.log,
            on_date(
                date!(2025 - 12 - 02),
                [
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:00:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None
                    },
                    TimeStamp {
                        typ: TimeStampType::End,
                        time: time!(11:03:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None
                    },
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:03:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: Some("foo".into())
                    }
                ],
            )
        );
    }

    #[test]
    fn insert_same_project_after_end_bump_time() {
        let initial = on_date(
            date!(2025 - 12 - 02),
            vec![
                TimeStamp {
                    typ: TimeStampType::Start,
                    time: time!(11:00:00),
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: None,
                },
                TimeStamp {
                    typ: TimeStampType::End,
                    time: time!(11:03:00),
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: None,
                },
            ],
        );
        let mut record = record!(2025-12-02, 11:04:00, initial);
        record.insert(String::new(), None);
        assert_eq!(
            record.log,
            on_date(
                date!(2025 - 12 - 02),
                vec![
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:00:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None,
                    },
                    TimeStamp {
                        typ: TimeStampType::End,
                        time: time!(11:04:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None,
                    },
                ],
            )
        );
    }

    #[test]
    fn insert_same_project_after_end_no_bump_time() {
        let initial = on_date(
            date!(2025 - 12 - 02),
            vec![
                TimeStamp {
                    typ: TimeStampType::Start,
                    time: time!(11:00:00),
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: None,
                },
                TimeStamp {
                    typ: TimeStampType::End,
                    time: time!(11:03:00),
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: None,
                },
            ],
        );
        let mut record = record!(2025-12-02, 11:44:00, initial);
        record.insert(String::new(), None);
        assert_eq!(
            record.log,
            on_date(
                date!(2025 - 12 - 02),
                vec![
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:00:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None,
                    },
                    TimeStamp {
                        typ: TimeStampType::End,
                        time: time!(11:03:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None,
                    },
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:44:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None,
                    },
                ],
            )
        );
    }

    #[test]
    fn insert_different_project_after_end_bump_time() {
        let initial = on_date(
            date!(2025 - 12 - 02),
            vec![
                TimeStamp {
                    typ: TimeStampType::Start,
                    time: time!(11:00:00),
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: None,
                },
                TimeStamp {
                    typ: TimeStampType::End,
                    time: time!(11:03:00),
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: None,
                },
            ],
        );
        let mut record = record!(2025-12-02, 11:04:00, initial);
        record.insert(String::new(), Some("foo".into()));
        assert_eq!(
            record.log,
            on_date(
                date!(2025 - 12 - 02),
                vec![
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:00:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None,
                    },
                    TimeStamp {
                        typ: TimeStampType::End,
                        time: time!(11:04:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None,
                    },
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:04:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: Some("foo".into()),
                    },
                ],
            )
        );
    }

    #[test]
    fn insert_different_project_after_end_no_bump_time() {
        let initial = on_date(
            date!(2025 - 12 - 02),
            vec![
                TimeStamp {
                    typ: TimeStampType::Start,
                    time: time!(11:00:00),
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: None,
                },
                TimeStamp {
                    typ: TimeStampType::End,
                    time: time!(11:03:00),
                    tolerance: DEFAULT_TOLERANCE,
                    sub_project: None,
                },
            ],
        );
        let mut record = record!(2025-12-02, 11:44:00, initial);
        record.insert(String::new(), Some("foo".into()));
        assert_eq!(
            record.log,
            on_date(
                date!(2025 - 12 - 02),
                vec![
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:00:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None,
                    },
                    TimeStamp {
                        typ: TimeStampType::End,
                        time: time!(11:03:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: None,
                    },
                    TimeStamp {
                        typ: TimeStampType::Start,
                        time: time!(11:44:00),
                        tolerance: DEFAULT_TOLERANCE,
                        sub_project: Some("foo".into()),
                    },
                ],
            )
        );
    }
}

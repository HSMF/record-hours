use std::{collections::BTreeMap, fmt::Display};

use chrono::Duration;

use crate::{Date, DecimalDuration};

pub struct Formatter<'a> {
    pub date: Date,
    pub times: &'a [super::Item],
    pub format: &'a str,
    pub project: &'a str,
}

impl Formatter<'_> {
    fn show_different_subprojects(&self) -> bool {
        let mut chars = self.format.chars();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                continue;
            }

            if chars.next() == Some('p') {
                return true;
            }
        }

        false
    }

    fn fmt_with_subproject(
        &self,
        subproject: &str,
        duration: Duration,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let mut chars = self.format.chars();

        while let Some(ch) = chars.next() {
            if ch != '%' {
                write!(f, "{ch}")?;
                continue;
            }

            match chars.next() {
                Some('%') => write!(f, "%")?,
                Some('d') => write!(f, "{}", self.date)?,
                Some('Y') => write!(f, "{}", self.date.0.format("%Y"))?,
                Some('M') => write!(f, "{}", self.date.0.format("%m"))?,
                Some('D') => write!(f, "{}", self.date.0.format("%d"))?,
                Some('t') => write!(f, "{}", DecimalDuration(duration))?,
                Some('h') => write!(f, "{}", duration.num_hours())?,
                Some('m') => write!(f, "{}", duration.num_minutes())?,
                Some('P') => write!(f, "{}", self.project)?, //the project
                Some('p') => write!(f, "{}", subproject)?,
                _ => return Err(std::fmt::Error),
            }
        }

        Ok(())
    }
}

impl Display for Formatter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.show_different_subprojects() {
            let durations =
                self.times
                    .iter()
                    .fold(BTreeMap::<_, Duration>::new(), |mut acc, item| {
                        let Some(end) = item.end else { return acc };
                        *acc.entry(item.project.as_ref().map_or("(-)", |x| x.as_str()))
                            .or_default() += end.0 - item.start.0;
                        acc
                    });
            for (proj, duration) in durations {
                self.fmt_with_subproject(proj, duration, f)?;
            }
            Ok(())
        } else {
            let duration: Duration = self
                .times
                .iter()
                .filter_map(|x| x.end.map(|end| end.0 - x.start.0))
                .sum();

            self.fmt_with_subproject("", duration, f)
        }
    }
}

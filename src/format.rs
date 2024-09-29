use std::fmt::Display;

use chrono::Duration;

use crate::{Date, DecimalDuration};

pub struct Formatter<'a> {
    pub date: Date,
    pub duration: Duration,
    pub format: &'a str,
    pub project: &'a str,
}

impl Display for Formatter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut chars = self.format.chars().peekable();

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
                Some('t') => write!(f, "{}", DecimalDuration(self.duration))?,
                Some('h') => write!(f, "{}", self.duration.num_hours())?,
                Some('m') => write!(f, "{}", self.duration.num_minutes())?,
                Some('P') => write!(f, "{}", self.project)?, //the project
                _ => return Err(std::fmt::Error),
            }
        }

        Ok(())
    }
}

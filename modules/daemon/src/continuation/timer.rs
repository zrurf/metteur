//! Deterministic schedule arithmetic; clock ownership stays with the daemon.
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Rule {
    Once {
        at: DateTime<Utc>,
    },
    After {
        deadline: DateTime<Utc>,
    },
    Interval {
        anchor: DateTime<Utc>,
        seconds: u64,
    },
    Daily {
        zone: String,
        hour: u32,
        minute: u32,
    },
    Weekly {
        zone: String,
        weekday: u32,
        hour: u32,
        minute: u32,
    },
}
impl Rule {
    pub fn next_after(&self, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, &'static str> {
        match self {
            Self::Once {
                at,
            }
            | Self::After {
                deadline: at,
            } => Ok((*at > after).then_some(*at)),
            Self::Interval {
                anchor,
                seconds,
            } => {
                if *seconds < 60 || *seconds > 31536000 {
                    return Err("Interval must be between 60 seconds and one year");
                }
                let seconds = *seconds as i64;
                let n = if after < *anchor {
                    0
                } else {
                    after.signed_duration_since(*anchor).num_seconds() / seconds + 1
                };
                Ok(anchor.checked_add_signed(Duration::seconds(
                    n.checked_mul(seconds).ok_or("Schedule overflow")?,
                )))
            }
            Self::Daily {
                zone,
                hour,
                minute,
            }
            | Self::Weekly {
                zone,
                hour,
                minute,
                ..
            } => {
                let zone: Tz = zone.parse().map_err(|_| "Unknown IANA timezone")?;
                let time =
                    NaiveTime::from_hms_opt(*hour, *minute, 0).ok_or("Invalid local time")?;
                if matches!(self,Self::Weekly{weekday,..} if *weekday>6) {
                    return Err("Invalid weekday");
                }
                let date = after.with_timezone(&zone).date_naive();
                // A bounded search covers skipped DST days and weekly schedules.
                for offset in 0..15 {
                    let day = date
                        .checked_add_signed(Duration::days(offset))
                        .ok_or("Schedule overflow")?;
                    if let Self::Weekly {
                        weekday,
                        ..
                    } = self
                    {
                        if day.weekday().num_days_from_monday() != *weekday {
                            continue;
                        }
                    }
                    let candidate = match zone.from_local_datetime(&day.and_time(time)) {
                        LocalResult::None => continue,
                        LocalResult::Single(t) => t.with_timezone(&Utc),
                        LocalResult::Ambiguous(a, b) => a.min(b).with_timezone(&Utc),
                    };
                    if candidate > after {
                        return Ok(Some(candidate));
                    }
                }
                Err("No calendar occurrence in bounded horizon")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }
    #[test]
    fn dst_gap_fold_and_intervals_are_deterministic() {
        let gap = Rule::Daily {
            zone: "America/New_York".into(),
            hour: 2,
            minute: 30,
        };
        assert_eq!(
            gap.next_after(at("2026-03-08T00:00:00Z")).unwrap(),
            Some(at("2026-03-09T06:30:00Z"))
        );
        let fold = Rule::Daily {
            zone: "America/New_York".into(),
            hour: 1,
            minute: 30,
        };
        assert_eq!(
            fold.next_after(at("2026-11-01T00:00:00Z")).unwrap(),
            Some(at("2026-11-01T05:30:00Z"))
        );
        assert_eq!(
            fold.next_after(at("2026-11-01T05:30:00Z")).unwrap(),
            Some(at("2026-11-02T06:30:00Z"))
        );
        let interval = Rule::Interval {
            anchor: at("2026-10-15T00:00:00Z"),
            seconds: 60,
        };
        assert_eq!(
            interval.next_after(at("2026-10-15T00:03:10Z")).unwrap(),
            Some(at("2026-10-15T00:04:00Z"))
        );
        assert!(
            Rule::Daily {
                zone: "Unknown/Zone".into(),
                hour: 1,
                minute: 0
            }
            .next_after(at("2026-10-15T00:00:00Z"))
            .is_err()
        );
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Misfire {
    CoalesceOnce,
    Skip,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub id: uuid::Uuid,
    pub generation: u64,
    pub task: uuid::Uuid,
    pub task_generation: u64,
    pub rule: Rule,
    pub next_due: Option<DateTime<Utc>>,
    pub last_emitted: Option<DateTime<Utc>>,
    pub max_occurrences: u32,
    pub occurrences: u32,
    pub expires_at: DateTime<Utc>,
    pub paused: bool,
    pub cancelled: bool,
    pub confirmed_digest: String,
    pub misfire: Misfire,
    pub params: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Occurrence {
    pub id: uuid::Uuid,
    pub due: DateTime<Utc>,
    pub missed_count: u64,
    pub schedule_generation: u64,
}
impl Schedule {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.max_occurrences == 0
            || self.max_occurrences > 10000
            || self.confirmed_digest.is_empty()
        {
            return Err("A bounded confirmed schedule is required");
        }
        if !self.params.is_object() || self.params.to_string().len() > 8192 {
            return Err("Invalid schedule parameters");
        }
        if self.next_due.is_some_and(|n| n > self.expires_at) {
            return Err("Schedule expires before its next occurrence");
        }
        Ok(())
    }
    /// Pure transition. Caller persists the updated plan and outbox atomically.
    pub fn advance(&mut self, now: DateTime<Utc>) -> Result<Option<Occurrence>, &'static str> {
        self.validate()?;
        if self.paused
            || self.cancelled
            || self.occurrences >= self.max_occurrences
            || now > self.expires_at
        {
            return Ok(None);
        }
        let Some(mut due) = self.next_due else {
            return Ok(None);
        };
        if due > now {
            return Ok(None);
        }
        let mut missed = 0u64;
        if let Rule::Interval {
            anchor,
            seconds,
        } = &self.rule
        {
            if *seconds < 60 || *seconds > 31536000 {
                return Err("Invalid fixed interval");
            }
            missed = now.signed_duration_since(due).num_seconds().max(0) as u64 / *seconds;
            due = due
                .checked_add_signed(Duration::seconds((missed * *seconds) as i64))
                .ok_or("Schedule overflow")?;
            if due < *anchor {
                return Err("Invalid interval anchor");
            }
            self.next_due = self.rule.next_after(now)?;
        } else {
            let mut next = self.rule.next_after(due)?;
            while let Some(n) = next {
                if n > now {
                    break;
                }
                missed += 1;
                if missed > 10000 {
                    return Err("Calendar catch-up requires review");
                }
                due = n;
                next = self.rule.next_after(n)?;
            }
            self.next_due = next;
        }
        if self.last_emitted.is_some_and(|last| due <= last) {
            return Ok(None);
        }
        // Skip missed occurrences, while a due occurrence observed on time can fire.
        if self.misfire == Misfire::Skip && now > due + Duration::seconds(1) {
            return Ok(None);
        }
        self.last_emitted = Some(due);
        self.occurrences += 1;
        if self.occurrences >= self.max_occurrences {
            self.next_due = None;
        }
        let id = uuid::Uuid::new_v5(
            &self.id,
            format!("{}:{}", self.generation, due.timestamp_millis()).as_bytes(),
        );
        Ok(Some(Occurrence {
            id,
            due,
            missed_count: missed,
            schedule_generation: self.generation,
        }))
    }
    pub fn cancel(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.cancelled = true;
        self.next_due = None;
    }
}

#[cfg(test)]
mod schedule_tests {
    use super::*;
    #[test]
    fn coalesce_skip_rollback_and_stop_preserve_occurrence_identity() {
        let anchor: DateTime<Utc> = "2026-10-15T00:00:00Z".parse().unwrap();
        let mut s = Schedule {
            id: uuid::Uuid::new_v4(),
            generation: 1,
            task: uuid::Uuid::new_v4(),
            task_generation: 1,
            rule: Rule::Interval {
                anchor,
                seconds: 60,
            },
            next_due: Some(anchor),
            last_emitted: None,
            max_occurrences: 3,
            occurrences: 0,
            expires_at: anchor + Duration::hours(1),
            paused: false,
            cancelled: false,
            confirmed_digest: "explicit-plan".into(),
            misfire: Misfire::CoalesceOnce,
            params: serde_json::json!({}),
        };
        let e = s.advance(anchor + Duration::seconds(185)).unwrap().unwrap();
        assert_eq!(e.missed_count, 3);
        assert_eq!(e.due, anchor + Duration::seconds(180));
        let persisted = serde_json::to_string(&s).unwrap();
        let mut restored: Schedule = serde_json::from_str(&persisted).unwrap();
        assert!(restored.advance(anchor + Duration::seconds(100)).unwrap().is_none());
        assert!(restored.advance(anchor + Duration::seconds(185)).unwrap().is_none());
        let e2 = restored.advance(anchor + Duration::seconds(240)).unwrap().unwrap();
        assert_ne!(e.id, e2.id);
        restored.cancel();
        assert!(restored.advance(anchor + Duration::seconds(300)).unwrap().is_none());
        s.misfire = Misfire::Skip;
        assert!(s.advance(anchor + Duration::seconds(245)).unwrap().is_none());
        assert_eq!(s.occurrences, 1);
    }
}

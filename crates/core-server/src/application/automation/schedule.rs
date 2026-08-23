use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
    Timelike, Utc, Weekday,
};
use chrono_tz::Tz;
use mycopilot_protocol_rs::{
    AutomationCustomFrequencyDto, AutomationIntervalUnitDto, AutomationScheduleDto,
    AutomationScheduleInputDto, AutomationWeekdayDto,
};
use std::error::Error;
use std::fmt::{Display, Formatter};

const MILLIS_PER_MINUTE: i64 = 60_000;
const MILLIS_PER_HOUR: i64 = 60 * MILLIS_PER_MINUTE;
const MILLIS_PER_DAY: i64 = 24 * MILLIS_PER_HOUR;
const MAX_CALENDAR_SEARCH_PERIODS: usize = 4_800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomationScheduleErrorCode {
    InvalidTimezone,
    InvalidAnchor,
    InvalidAfter,
    InvalidInterval,
    InvalidTime,
    MissingField,
    UnexpectedField,
    EmptySelection,
    InvalidMonthDay,
    InvalidMonth,
    NextRunOutOfRange,
    NoFutureOccurrence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutomationScheduleError {
    pub(crate) code: AutomationScheduleErrorCode,
    pub(crate) field: &'static str,
    pub(crate) message: String,
}

impl AutomationScheduleError {
    fn new(
        code: AutomationScheduleErrorCode,
        field: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            field,
            message: message.into(),
        }
    }
}

impl Display for AutomationScheduleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.field, self.message)
    }
}

impl Error for AutomationScheduleError {}

/// Backend-normalized schedule used by both CRUD validation and the round-2 scheduler.
///
/// `schedule` contains only canonical timezone names and sorted, de-duplicated selections.
/// `rrule` is a stable RFC 5545 recurrence rule. `anchor_at` remains separate because RRULE does
/// not carry DTSTART and the database stores the anchor as an explicit authority field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizedAutomationSchedule {
    pub(crate) schedule: AutomationScheduleDto,
    pub(crate) rrule: String,
    pub(crate) timezone: Tz,
    pub(crate) anchor_at: i64,
    rule: NormalizedRule,
}

impl NormalizedAutomationSchedule {
    /// Stable, locale-neutral v1 description for list/detail rendering. The structured schedule
    /// remains authoritative; clients must not parse this text back into a rule.
    pub(crate) fn summary(&self) -> String {
        schedule_summary(&self.schedule)
    }

    /// Returns the first occurrence strictly later than `after_ms` and never earlier than the
    /// schedule anchor.
    pub(crate) fn next_run_at_ms(&self, after_ms: i64) -> Result<i64, AutomationScheduleError> {
        let after = utc_from_ms(
            after_ms,
            "afterMs",
            AutomationScheduleErrorCode::InvalidAfter,
        )?;
        match &self.rule {
            NormalizedRule::FixedInterval { step_ms } => {
                next_fixed_interval(self.anchor_at, after_ms, *step_ms)
            }
            NormalizedRule::Hourly {
                interval,
                minute_of_hour,
            } => self.next_hourly(after, *interval, *minute_of_hour),
            NormalizedRule::Daily {
                interval,
                time_minutes,
            } => self.next_daily(after, *interval, *time_minutes),
            NormalizedRule::Weekly {
                interval,
                weekdays,
                time_minutes,
            } => self.next_weekly(after, *interval, weekdays, *time_minutes),
            NormalizedRule::Monthly {
                interval,
                month_days,
                time_minutes,
            } => self.next_monthly(after, *interval, month_days, *time_minutes),
            NormalizedRule::Yearly {
                interval,
                months,
                month_days,
                time_minutes,
            } => self.next_yearly(after, *interval, months, month_days, *time_minutes),
        }
    }

    fn anchor_utc(&self) -> Result<DateTime<Utc>, AutomationScheduleError> {
        utc_from_ms(
            self.anchor_at,
            "anchorAt",
            AutomationScheduleErrorCode::InvalidAnchor,
        )
    }

    fn next_hourly(
        &self,
        after: DateTime<Utc>,
        interval: u32,
        minute_of_hour: u8,
    ) -> Result<i64, AutomationScheduleError> {
        let anchor = self.anchor_utc()?;
        let anchor_local = anchor.with_timezone(&self.timezone).naive_local();
        let anchor_hour = anchor_local
            .date()
            .and_hms_opt(anchor_local.hour(), 0, 0)
            .ok_or_else(next_run_out_of_range)?;
        let threshold = if after.timestamp_millis() < self.anchor_at {
            anchor
        } else {
            after
        };
        let threshold_local = threshold.with_timezone(&self.timezone).naive_local();
        let threshold_hour = threshold_local
            .date()
            .and_hms_opt(threshold_local.hour(), 0, 0)
            .ok_or_else(next_run_out_of_range)?;
        let interval = i64::from(interval);
        let elapsed_hours = threshold_hour
            .signed_duration_since(anchor_hour)
            .num_hours()
            .max(0);
        let mut period = elapsed_hours.div_euclid(interval) * interval;

        for _ in 0..MAX_CALENDAR_SEARCH_PERIODS {
            let hour = anchor_hour
                .checked_add_signed(Duration::hours(period))
                .ok_or_else(next_run_out_of_range)?;
            let candidate = hour
                .date()
                .and_hms_opt(hour.hour(), u32::from(minute_of_hour), 0)
                .ok_or_else(next_run_out_of_range)?;
            if let Some(candidate_ms) = resolve_local_once(self.timezone, candidate) {
                if self.is_eligible(candidate_ms, after.timestamp_millis()) {
                    return Ok(candidate_ms);
                }
            }
            period = period
                .checked_add(interval)
                .ok_or_else(next_run_out_of_range)?;
        }

        Err(no_future_occurrence())
    }

    fn next_daily(
        &self,
        after: DateTime<Utc>,
        interval: u32,
        time_minutes: u16,
    ) -> Result<i64, AutomationScheduleError> {
        let anchor = self.anchor_utc()?;
        let anchor_date = anchor.with_timezone(&self.timezone).date_naive();
        let threshold = if after.timestamp_millis() < self.anchor_at {
            anchor
        } else {
            after
        };
        let threshold_date = threshold.with_timezone(&self.timezone).date_naive();
        let interval = i64::from(interval);
        let elapsed_days = threshold_date
            .signed_duration_since(anchor_date)
            .num_days()
            .max(0);
        let mut period = elapsed_days.div_euclid(interval) * interval;
        let time = time_from_minutes(time_minutes)?;

        // A large custom interval can repeatedly land on a DST transition. Search a bounded
        // Gregorian cycle rather than assuming that the immediately following period is valid.
        for _ in 0..MAX_CALENDAR_SEARCH_PERIODS {
            let date = anchor_date
                .checked_add_signed(Duration::days(period))
                .ok_or_else(next_run_out_of_range)?;
            if let Some(candidate_ms) = resolve_local_once(self.timezone, date.and_time(time)) {
                if self.is_eligible(candidate_ms, after.timestamp_millis()) {
                    return Ok(candidate_ms);
                }
            }
            period = period
                .checked_add(interval)
                .ok_or_else(next_run_out_of_range)?;
        }

        Err(no_future_occurrence())
    }

    fn next_weekly(
        &self,
        after: DateTime<Utc>,
        interval: u32,
        weekdays: &[Weekday],
        time_minutes: u16,
    ) -> Result<i64, AutomationScheduleError> {
        let anchor = self.anchor_utc()?;
        let anchor_date = anchor.with_timezone(&self.timezone).date_naive();
        let anchor_week = monday_of_week(anchor_date)?;
        let threshold = if after.timestamp_millis() < self.anchor_at {
            anchor
        } else {
            after
        };
        let threshold_date = threshold.with_timezone(&self.timezone).date_naive();
        let elapsed_weeks = threshold_date
            .signed_duration_since(anchor_week)
            .num_days()
            .div_euclid(7)
            .max(0);
        let interval = i64::from(interval);
        let mut period_week = elapsed_weeks.div_euclid(interval) * interval;
        let time = time_from_minutes(time_minutes)?;

        for _ in 0..MAX_CALENDAR_SEARCH_PERIODS {
            let week = anchor_week
                .checked_add_signed(Duration::weeks(period_week))
                .ok_or_else(next_run_out_of_range)?;
            for weekday in weekdays {
                let date = week
                    .checked_add_signed(Duration::days(i64::from(weekday.num_days_from_monday())))
                    .ok_or_else(next_run_out_of_range)?;
                if let Some(candidate_ms) = resolve_local_once(self.timezone, date.and_time(time)) {
                    if self.is_eligible(candidate_ms, after.timestamp_millis()) {
                        return Ok(candidate_ms);
                    }
                }
            }
            period_week = period_week
                .checked_add(interval)
                .ok_or_else(next_run_out_of_range)?;
        }

        Err(no_future_occurrence())
    }

    fn next_monthly(
        &self,
        after: DateTime<Utc>,
        interval: u32,
        month_days: &[u8],
        time_minutes: u16,
    ) -> Result<i64, AutomationScheduleError> {
        let anchor = self.anchor_utc()?;
        let anchor_local = anchor.with_timezone(&self.timezone);
        let anchor_month = month_index(anchor_local.year(), anchor_local.month());
        let threshold = if after.timestamp_millis() < self.anchor_at {
            anchor
        } else {
            after
        };
        let threshold_local = threshold.with_timezone(&self.timezone);
        let threshold_month = month_index(threshold_local.year(), threshold_local.month());
        let interval = i64::from(interval);
        let elapsed_months = threshold_month.saturating_sub(anchor_month).max(0);
        let mut period = elapsed_months.div_euclid(interval) * interval;
        let time = time_from_minutes(time_minutes)?;

        // The Gregorian calendar repeats every 4,800 months. If no selected day is valid over
        // that cycle, this cadence has no future occurrence (for example every February 31st).
        for _ in 0..MAX_CALENDAR_SEARCH_PERIODS {
            let index = anchor_month
                .checked_add(period)
                .ok_or_else(next_run_out_of_range)?;
            let (year, month) = year_month_from_index(index)?;
            for day in month_days {
                let Some(date) = NaiveDate::from_ymd_opt(year, month, u32::from(*day)) else {
                    continue;
                };
                if let Some(candidate_ms) = resolve_local_once(self.timezone, date.and_time(time)) {
                    if self.is_eligible(candidate_ms, after.timestamp_millis()) {
                        return Ok(candidate_ms);
                    }
                }
            }
            period = period
                .checked_add(interval)
                .ok_or_else(next_run_out_of_range)?;
        }

        Err(no_future_occurrence())
    }

    fn next_yearly(
        &self,
        after: DateTime<Utc>,
        interval: u32,
        months: &[u8],
        month_days: &[u8],
        time_minutes: u16,
    ) -> Result<i64, AutomationScheduleError> {
        let anchor = self.anchor_utc()?;
        let anchor_year = i64::from(anchor.with_timezone(&self.timezone).year());
        let threshold = if after.timestamp_millis() < self.anchor_at {
            anchor
        } else {
            after
        };
        let threshold_year = i64::from(threshold.with_timezone(&self.timezone).year());
        let interval = i64::from(interval);
        let elapsed_years = threshold_year.saturating_sub(anchor_year).max(0);
        let mut period = elapsed_years.div_euclid(interval) * interval;
        let time = time_from_minutes(time_minutes)?;

        // Leap-year/date validity repeats every 400 years. Use the wider shared limit so very
        // large intervals still fail closed on DateTime overflow instead of looping forever.
        for _ in 0..MAX_CALENDAR_SEARCH_PERIODS {
            let year_i64 = anchor_year
                .checked_add(period)
                .ok_or_else(next_run_out_of_range)?;
            let year = i32::try_from(year_i64).map_err(|_| next_run_out_of_range())?;
            for month in months {
                for day in month_days {
                    let Some(date) =
                        NaiveDate::from_ymd_opt(year, u32::from(*month), u32::from(*day))
                    else {
                        continue;
                    };
                    if let Some(candidate_ms) =
                        resolve_local_once(self.timezone, date.and_time(time))
                    {
                        if self.is_eligible(candidate_ms, after.timestamp_millis()) {
                            return Ok(candidate_ms);
                        }
                    }
                }
            }
            period = period
                .checked_add(interval)
                .ok_or_else(next_run_out_of_range)?;
        }

        Err(no_future_occurrence())
    }

    fn is_eligible(&self, candidate_ms: i64, after_ms: i64) -> bool {
        candidate_ms >= self.anchor_at && candidate_ms > after_ms
    }
}

fn schedule_summary(schedule: &AutomationScheduleDto) -> String {
    match schedule {
        AutomationScheduleInputDto::Interval {
            amount,
            unit,
            timezone: _,
            anchor_at: _,
        } => {
            let unit = match unit {
                AutomationIntervalUnitDto::Minutes => plural(*amount, "minute", "minutes"),
                AutomationIntervalUnitDto::Hours => plural(*amount, "hour", "hours"),
                AutomationIntervalUnitDto::Days => plural(*amount, "day", "days"),
            };
            format!("Every {amount} {unit} (fixed UTC interval)")
        }
        AutomationScheduleInputDto::Daily {
            time_minutes,
            timezone,
            ..
        } => format!("Every day at {} ({timezone})", display_time(*time_minutes)),
        AutomationScheduleInputDto::Weekdays {
            time_minutes,
            timezone,
            ..
        } => format!("Weekdays at {} ({timezone})", display_time(*time_minutes)),
        AutomationScheduleInputDto::Weekly {
            weekdays,
            time_minutes,
            timezone,
            ..
        } => format!(
            "Every week on {} at {} ({timezone})",
            display_weekdays(weekdays),
            display_time(*time_minutes)
        ),
        AutomationScheduleInputDto::Custom {
            frequency,
            interval,
            minute_of_hour,
            time_minutes,
            weekdays,
            month_days,
            months,
            timezone,
            ..
        } => match frequency {
            AutomationCustomFrequencyDto::Hourly => format!(
                "Every {interval} {} at minute {} ({timezone})",
                plural(*interval, "hour", "hours"),
                minute_of_hour.unwrap_or_default()
            ),
            AutomationCustomFrequencyDto::Daily => format!(
                "Every {interval} {} at {} ({timezone})",
                plural(*interval, "day", "days"),
                display_time(time_minutes.unwrap_or_default())
            ),
            AutomationCustomFrequencyDto::Weekly => format!(
                "Every {interval} {} on {} at {} ({timezone})",
                plural(*interval, "week", "weeks"),
                display_weekdays(weekdays.as_deref().unwrap_or_default()),
                display_time(time_minutes.unwrap_or_default())
            ),
            AutomationCustomFrequencyDto::Monthly => format!(
                "Every {interval} {} on day {} at {} ({timezone})",
                plural(*interval, "month", "months"),
                display_numbers(month_days.as_deref().unwrap_or_default()),
                display_time(time_minutes.unwrap_or_default())
            ),
            AutomationCustomFrequencyDto::Yearly => format!(
                "Every {interval} {} in month {} on day {} at {} ({timezone})",
                plural(*interval, "year", "years"),
                display_numbers(months.as_deref().unwrap_or_default()),
                display_numbers(month_days.as_deref().unwrap_or_default()),
                display_time(time_minutes.unwrap_or_default())
            ),
        },
    }
}

fn plural(value: u32, singular: &'static str, plural: &'static str) -> &'static str {
    if value == 1 {
        singular
    } else {
        plural
    }
}

fn display_time(time_minutes: u16) -> String {
    format!("{:02}:{:02}", time_minutes / 60, time_minutes % 60)
}

fn display_weekdays(weekdays: &[AutomationWeekdayDto]) -> String {
    weekdays
        .iter()
        .map(|weekday| match weekday {
            AutomationWeekdayDto::Monday => "Mon",
            AutomationWeekdayDto::Tuesday => "Tue",
            AutomationWeekdayDto::Wednesday => "Wed",
            AutomationWeekdayDto::Thursday => "Thu",
            AutomationWeekdayDto::Friday => "Fri",
            AutomationWeekdayDto::Saturday => "Sat",
            AutomationWeekdayDto::Sunday => "Sun",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn display_numbers(values: &[u8]) -> String {
    values
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NormalizedRule {
    FixedInterval {
        step_ms: i64,
    },
    Hourly {
        interval: u32,
        minute_of_hour: u8,
    },
    Daily {
        interval: u32,
        time_minutes: u16,
    },
    Weekly {
        interval: u32,
        weekdays: Vec<Weekday>,
        time_minutes: u16,
    },
    Monthly {
        interval: u32,
        month_days: Vec<u8>,
        time_minutes: u16,
    },
    Yearly {
        interval: u32,
        months: Vec<u8>,
        month_days: Vec<u8>,
        time_minutes: u16,
    },
}

pub(crate) fn normalize_schedule(
    input: &AutomationScheduleInputDto,
) -> Result<NormalizedAutomationSchedule, AutomationScheduleError> {
    match input {
        AutomationScheduleInputDto::Interval {
            amount,
            unit,
            anchor_at,
            timezone,
        } => {
            validate_positive(*amount, "amount")?;
            validate_anchor(*anchor_at)?;
            let timezone = parse_timezone(timezone)?;
            let unit_ms = match unit {
                AutomationIntervalUnitDto::Minutes => MILLIS_PER_MINUTE,
                AutomationIntervalUnitDto::Hours => MILLIS_PER_HOUR,
                AutomationIntervalUnitDto::Days => MILLIS_PER_DAY,
            };
            let step_ms = i64::from(*amount)
                .checked_mul(unit_ms)
                .ok_or_else(|| invalid_interval("interval is too large"))?;
            let frequency = match unit {
                AutomationIntervalUnitDto::Minutes => "MINUTELY",
                AutomationIntervalUnitDto::Hours => "HOURLY",
                AutomationIntervalUnitDto::Days => "DAILY",
            };
            Ok(NormalizedAutomationSchedule {
                schedule: AutomationScheduleInputDto::Interval {
                    amount: *amount,
                    unit: *unit,
                    anchor_at: *anchor_at,
                    timezone: timezone.to_string(),
                },
                rrule: format!("FREQ={frequency};INTERVAL={amount}"),
                timezone,
                anchor_at: *anchor_at,
                rule: NormalizedRule::FixedInterval { step_ms },
            })
        }
        AutomationScheduleInputDto::Daily {
            time_minutes,
            anchor_at,
            timezone,
        } => normalize_daily(1, *time_minutes, *anchor_at, timezone, false),
        AutomationScheduleInputDto::Weekdays {
            time_minutes,
            anchor_at,
            timezone,
        } => normalize_weekly(
            1,
            vec![
                AutomationWeekdayDto::Monday,
                AutomationWeekdayDto::Tuesday,
                AutomationWeekdayDto::Wednesday,
                AutomationWeekdayDto::Thursday,
                AutomationWeekdayDto::Friday,
            ],
            *time_minutes,
            *anchor_at,
            timezone,
            WeeklyInputKind::Weekdays,
        ),
        AutomationScheduleInputDto::Weekly {
            weekdays,
            time_minutes,
            anchor_at,
            timezone,
        } => normalize_weekly(
            1,
            weekdays.clone(),
            *time_minutes,
            *anchor_at,
            timezone,
            WeeklyInputKind::Weekly,
        ),
        AutomationScheduleInputDto::Custom {
            frequency,
            interval,
            minute_of_hour,
            time_minutes,
            weekdays,
            month_days,
            months,
            anchor_at,
            timezone,
        } => {
            validate_positive(*interval, "interval")?;
            validate_anchor(*anchor_at)?;
            match frequency {
                AutomationCustomFrequencyDto::Hourly => {
                    reject_present(time_minutes, "timeMinutes", "hourly")?;
                    reject_present(weekdays, "weekdays", "hourly")?;
                    reject_present(month_days, "monthDays", "hourly")?;
                    reject_present(months, "months", "hourly")?;
                    let minute = require(*minute_of_hour, "minuteOfHour", "hourly")?;
                    normalize_hourly(*interval, minute, *anchor_at, timezone)
                }
                AutomationCustomFrequencyDto::Daily => {
                    reject_present(minute_of_hour, "minuteOfHour", "daily")?;
                    reject_present(weekdays, "weekdays", "daily")?;
                    reject_present(month_days, "monthDays", "daily")?;
                    reject_present(months, "months", "daily")?;
                    let time = require(*time_minutes, "timeMinutes", "daily")?;
                    normalize_daily(*interval, time, *anchor_at, timezone, true)
                }
                AutomationCustomFrequencyDto::Weekly => {
                    reject_present(minute_of_hour, "minuteOfHour", "weekly")?;
                    reject_present(month_days, "monthDays", "weekly")?;
                    reject_present(months, "months", "weekly")?;
                    let time = require(*time_minutes, "timeMinutes", "weekly")?;
                    let weekdays = require(weekdays.clone(), "weekdays", "weekly")?;
                    normalize_weekly(
                        *interval,
                        weekdays,
                        time,
                        *anchor_at,
                        timezone,
                        WeeklyInputKind::Custom,
                    )
                }
                AutomationCustomFrequencyDto::Monthly => {
                    reject_present(minute_of_hour, "minuteOfHour", "monthly")?;
                    reject_present(weekdays, "weekdays", "monthly")?;
                    reject_present(months, "months", "monthly")?;
                    let time = require(*time_minutes, "timeMinutes", "monthly")?;
                    let days = require(month_days.clone(), "monthDays", "monthly")?;
                    normalize_monthly(*interval, days, time, *anchor_at, timezone)
                }
                AutomationCustomFrequencyDto::Yearly => {
                    reject_present(minute_of_hour, "minuteOfHour", "yearly")?;
                    reject_present(weekdays, "weekdays", "yearly")?;
                    let time = require(*time_minutes, "timeMinutes", "yearly")?;
                    let days = require(month_days.clone(), "monthDays", "yearly")?;
                    let months = require(months.clone(), "months", "yearly")?;
                    normalize_yearly(*interval, months, days, time, *anchor_at, timezone)
                }
            }
        }
    }
}

fn normalize_hourly(
    interval: u32,
    minute_of_hour: u8,
    anchor_at: i64,
    timezone: &str,
) -> Result<NormalizedAutomationSchedule, AutomationScheduleError> {
    if minute_of_hour > 59 {
        return Err(AutomationScheduleError::new(
            AutomationScheduleErrorCode::InvalidTime,
            "minuteOfHour",
            "minute must be between 0 and 59",
        ));
    }
    let timezone = parse_timezone(timezone)?;
    Ok(NormalizedAutomationSchedule {
        schedule: AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Hourly,
            interval,
            minute_of_hour: Some(minute_of_hour),
            time_minutes: None,
            weekdays: None,
            month_days: None,
            months: None,
            anchor_at,
            timezone: timezone.to_string(),
        },
        rrule: format!("FREQ=HOURLY;INTERVAL={interval};BYMINUTE={minute_of_hour};BYSECOND=0"),
        timezone,
        anchor_at,
        rule: NormalizedRule::Hourly {
            interval,
            minute_of_hour,
        },
    })
}

fn normalize_daily(
    interval: u32,
    time_minutes: u16,
    anchor_at: i64,
    timezone: &str,
    custom: bool,
) -> Result<NormalizedAutomationSchedule, AutomationScheduleError> {
    validate_positive(interval, "interval")?;
    validate_anchor(anchor_at)?;
    let (hour, minute) = validate_time_minutes(time_minutes)?;
    let timezone = parse_timezone(timezone)?;
    let schedule = if custom {
        AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Daily,
            interval,
            minute_of_hour: None,
            time_minutes: Some(time_minutes),
            weekdays: None,
            month_days: None,
            months: None,
            anchor_at,
            timezone: timezone.to_string(),
        }
    } else {
        AutomationScheduleInputDto::Daily {
            time_minutes,
            anchor_at,
            timezone: timezone.to_string(),
        }
    };
    Ok(NormalizedAutomationSchedule {
        schedule,
        rrule: wall_time_rrule("DAILY", interval, None, None, None, hour, minute),
        timezone,
        anchor_at,
        rule: NormalizedRule::Daily {
            interval,
            time_minutes,
        },
    })
}

#[derive(Clone, Copy)]
enum WeeklyInputKind {
    Weekdays,
    Weekly,
    Custom,
}

fn normalize_weekly(
    interval: u32,
    weekdays: Vec<AutomationWeekdayDto>,
    time_minutes: u16,
    anchor_at: i64,
    timezone: &str,
    input_kind: WeeklyInputKind,
) -> Result<NormalizedAutomationSchedule, AutomationScheduleError> {
    validate_positive(interval, "interval")?;
    validate_anchor(anchor_at)?;
    let (hour, minute) = validate_time_minutes(time_minutes)?;
    let timezone = parse_timezone(timezone)?;
    let weekdays = normalize_weekdays(weekdays)?;
    let chrono_weekdays = weekdays.iter().copied().map(to_chrono_weekday).collect();
    let by_day = weekdays
        .iter()
        .copied()
        .map(rrule_weekday)
        .collect::<Vec<_>>()
        .join(",");
    let schedule = match input_kind {
        WeeklyInputKind::Weekdays => AutomationScheduleInputDto::Weekdays {
            time_minutes,
            anchor_at,
            timezone: timezone.to_string(),
        },
        WeeklyInputKind::Weekly => AutomationScheduleInputDto::Weekly {
            weekdays,
            time_minutes,
            anchor_at,
            timezone: timezone.to_string(),
        },
        WeeklyInputKind::Custom => AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Weekly,
            interval,
            minute_of_hour: None,
            time_minutes: Some(time_minutes),
            weekdays: Some(weekdays),
            month_days: None,
            months: None,
            anchor_at,
            timezone: timezone.to_string(),
        },
    };
    Ok(NormalizedAutomationSchedule {
        schedule,
        rrule: wall_time_rrule("WEEKLY", interval, Some(&by_day), None, None, hour, minute)
            + ";WKST=MO",
        timezone,
        anchor_at,
        rule: NormalizedRule::Weekly {
            interval,
            weekdays: chrono_weekdays,
            time_minutes,
        },
    })
}

fn normalize_monthly(
    interval: u32,
    month_days: Vec<u8>,
    time_minutes: u16,
    anchor_at: i64,
    timezone: &str,
) -> Result<NormalizedAutomationSchedule, AutomationScheduleError> {
    let (hour, minute) = validate_time_minutes(time_minutes)?;
    let month_days = normalize_u8_selection(month_days, 1, 31, "monthDays")?;
    let timezone = parse_timezone(timezone)?;
    let by_month_day = join_numbers(&month_days);
    Ok(NormalizedAutomationSchedule {
        schedule: AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Monthly,
            interval,
            minute_of_hour: None,
            time_minutes: Some(time_minutes),
            weekdays: None,
            month_days: Some(month_days.clone()),
            months: None,
            anchor_at,
            timezone: timezone.to_string(),
        },
        rrule: wall_time_rrule(
            "MONTHLY",
            interval,
            None,
            None,
            Some(&by_month_day),
            hour,
            minute,
        ),
        timezone,
        anchor_at,
        rule: NormalizedRule::Monthly {
            interval,
            month_days,
            time_minutes,
        },
    })
}

fn normalize_yearly(
    interval: u32,
    months: Vec<u8>,
    month_days: Vec<u8>,
    time_minutes: u16,
    anchor_at: i64,
    timezone: &str,
) -> Result<NormalizedAutomationSchedule, AutomationScheduleError> {
    let (hour, minute) = validate_time_minutes(time_minutes)?;
    let months = normalize_u8_selection(months, 1, 12, "months")?;
    let month_days = normalize_u8_selection(month_days, 1, 31, "monthDays")?;
    let timezone = parse_timezone(timezone)?;
    let by_month = join_numbers(&months);
    let by_month_day = join_numbers(&month_days);
    Ok(NormalizedAutomationSchedule {
        schedule: AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Yearly,
            interval,
            minute_of_hour: None,
            time_minutes: Some(time_minutes),
            weekdays: None,
            month_days: Some(month_days.clone()),
            months: Some(months.clone()),
            anchor_at,
            timezone: timezone.to_string(),
        },
        rrule: wall_time_rrule(
            "YEARLY",
            interval,
            None,
            Some(&by_month),
            Some(&by_month_day),
            hour,
            minute,
        ),
        timezone,
        anchor_at,
        rule: NormalizedRule::Yearly {
            interval,
            months,
            month_days,
            time_minutes,
        },
    })
}

fn wall_time_rrule(
    frequency: &str,
    interval: u32,
    by_day: Option<&str>,
    by_month: Option<&str>,
    by_month_day: Option<&str>,
    hour: u16,
    minute: u16,
) -> String {
    let mut parts = vec![format!("FREQ={frequency}"), format!("INTERVAL={interval}")];
    if let Some(value) = by_day {
        parts.push(format!("BYDAY={value}"));
    }
    if let Some(value) = by_month {
        parts.push(format!("BYMONTH={value}"));
    }
    if let Some(value) = by_month_day {
        parts.push(format!("BYMONTHDAY={value}"));
    }
    parts.push(format!("BYHOUR={hour}"));
    parts.push(format!("BYMINUTE={minute}"));
    parts.push("BYSECOND=0".to_string());
    parts.join(";")
}

fn validate_positive(value: u32, field: &'static str) -> Result<(), AutomationScheduleError> {
    if value == 0 {
        return Err(AutomationScheduleError::new(
            AutomationScheduleErrorCode::InvalidInterval,
            field,
            "interval must be at least 1",
        ));
    }
    Ok(())
}

fn validate_anchor(anchor_at: i64) -> Result<(), AutomationScheduleError> {
    utc_from_ms(
        anchor_at,
        "anchorAt",
        AutomationScheduleErrorCode::InvalidAnchor,
    )
    .map(|_| ())
}

fn parse_timezone(timezone: &str) -> Result<Tz, AutomationScheduleError> {
    let timezone = timezone.trim();
    if timezone.is_empty() {
        return Err(AutomationScheduleError::new(
            AutomationScheduleErrorCode::InvalidTimezone,
            "timezone",
            "IANA timezone is required",
        ));
    }
    timezone.parse::<Tz>().map_err(|_| {
        AutomationScheduleError::new(
            AutomationScheduleErrorCode::InvalidTimezone,
            "timezone",
            "timezone must be a recognized IANA name",
        )
    })
}

fn validate_time_minutes(time_minutes: u16) -> Result<(u16, u16), AutomationScheduleError> {
    if time_minutes >= 24 * 60 {
        return Err(AutomationScheduleError::new(
            AutomationScheduleErrorCode::InvalidTime,
            "timeMinutes",
            "time must be between 00:00 and 23:59",
        ));
    }
    Ok((time_minutes / 60, time_minutes % 60))
}

fn time_from_minutes(time_minutes: u16) -> Result<NaiveTime, AutomationScheduleError> {
    let (hour, minute) = validate_time_minutes(time_minutes)?;
    NaiveTime::from_hms_opt(u32::from(hour), u32::from(minute), 0).ok_or_else(|| {
        AutomationScheduleError::new(
            AutomationScheduleErrorCode::InvalidTime,
            "timeMinutes",
            "time cannot be represented",
        )
    })
}

fn require<T>(
    value: Option<T>,
    field: &'static str,
    frequency: &str,
) -> Result<T, AutomationScheduleError> {
    value.ok_or_else(|| {
        AutomationScheduleError::new(
            AutomationScheduleErrorCode::MissingField,
            field,
            format!("{field} is required for {frequency} schedules"),
        )
    })
}

fn reject_present<T>(
    value: &Option<T>,
    field: &'static str,
    frequency: &str,
) -> Result<(), AutomationScheduleError> {
    if value.is_some() {
        return Err(AutomationScheduleError::new(
            AutomationScheduleErrorCode::UnexpectedField,
            field,
            format!("{field} is not valid for {frequency} schedules"),
        ));
    }
    Ok(())
}

fn normalize_weekdays(
    weekdays: Vec<AutomationWeekdayDto>,
) -> Result<Vec<AutomationWeekdayDto>, AutomationScheduleError> {
    if weekdays.is_empty() {
        return Err(AutomationScheduleError::new(
            AutomationScheduleErrorCode::EmptySelection,
            "weekdays",
            "at least one weekday is required",
        ));
    }
    let mut weekdays = weekdays;
    weekdays.sort_by_key(|weekday| weekday_order(*weekday));
    weekdays.dedup();
    Ok(weekdays)
}

fn normalize_u8_selection(
    mut values: Vec<u8>,
    minimum: u8,
    maximum: u8,
    field: &'static str,
) -> Result<Vec<u8>, AutomationScheduleError> {
    if values.is_empty() {
        return Err(AutomationScheduleError::new(
            AutomationScheduleErrorCode::EmptySelection,
            field,
            format!("at least one {field} value is required"),
        ));
    }
    if values
        .iter()
        .any(|value| *value < minimum || *value > maximum)
    {
        let code = if field == "months" {
            AutomationScheduleErrorCode::InvalidMonth
        } else {
            AutomationScheduleErrorCode::InvalidMonthDay
        };
        return Err(AutomationScheduleError::new(
            code,
            field,
            format!("values must be between {minimum} and {maximum}"),
        ));
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

fn next_fixed_interval(
    anchor_at: i64,
    after_ms: i64,
    step_ms: i64,
) -> Result<i64, AutomationScheduleError> {
    if after_ms < anchor_at {
        return Ok(anchor_at);
    }
    let elapsed = after_ms
        .checked_sub(anchor_at)
        .ok_or_else(next_run_out_of_range)?;
    let periods = elapsed
        .div_euclid(step_ms)
        .checked_add(1)
        .ok_or_else(next_run_out_of_range)?;
    let offset = periods
        .checked_mul(step_ms)
        .ok_or_else(next_run_out_of_range)?;
    let candidate = anchor_at
        .checked_add(offset)
        .ok_or_else(next_run_out_of_range)?;
    utc_from_ms(
        candidate,
        "nextRunAt",
        AutomationScheduleErrorCode::NextRunOutOfRange,
    )?;
    Ok(candidate)
}

fn resolve_local_once(timezone: Tz, local: NaiveDateTime) -> Option<i64> {
    match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) => Some(value.timestamp_millis()),
        // The earlier absolute instant is the single authoritative occurrence during a fall-back
        // fold. Once that instant passes, callers advance to the next recurrence instead of
        // running the same wall-clock occurrence twice.
        LocalResult::Ambiguous(first, second) => {
            Some(first.timestamp_millis().min(second.timestamp_millis()))
        }
        // A local time inside a spring-forward gap does not exist and is deliberately skipped.
        LocalResult::None => None,
    }
}

fn monday_of_week(date: NaiveDate) -> Result<NaiveDate, AutomationScheduleError> {
    date.checked_sub_signed(Duration::days(i64::from(
        date.weekday().num_days_from_monday(),
    )))
    .ok_or_else(next_run_out_of_range)
}

fn month_index(year: i32, month: u32) -> i64 {
    i64::from(year) * 12 + i64::from(month - 1)
}

fn year_month_from_index(index: i64) -> Result<(i32, u32), AutomationScheduleError> {
    let year = i32::try_from(index.div_euclid(12)).map_err(|_| next_run_out_of_range())?;
    let month = u32::try_from(index.rem_euclid(12) + 1).map_err(|_| next_run_out_of_range())?;
    Ok((year, month))
}

fn utc_from_ms(
    value: i64,
    field: &'static str,
    code: AutomationScheduleErrorCode,
) -> Result<DateTime<Utc>, AutomationScheduleError> {
    DateTime::<Utc>::from_timestamp_millis(value).ok_or_else(|| {
        AutomationScheduleError::new(code, field, "timestamp is outside the supported range")
    })
}

fn invalid_interval(message: impl Into<String>) -> AutomationScheduleError {
    AutomationScheduleError::new(
        AutomationScheduleErrorCode::InvalidInterval,
        "interval",
        message,
    )
}

fn next_run_out_of_range() -> AutomationScheduleError {
    AutomationScheduleError::new(
        AutomationScheduleErrorCode::NextRunOutOfRange,
        "nextRunAt",
        "next occurrence is outside the supported timestamp range",
    )
}

fn no_future_occurrence() -> AutomationScheduleError {
    AutomationScheduleError::new(
        AutomationScheduleErrorCode::NoFutureOccurrence,
        "schedule",
        "schedule has no future occurrence in the supported calendar range",
    )
}

fn weekday_order(value: AutomationWeekdayDto) -> u8 {
    match value {
        AutomationWeekdayDto::Monday => 0,
        AutomationWeekdayDto::Tuesday => 1,
        AutomationWeekdayDto::Wednesday => 2,
        AutomationWeekdayDto::Thursday => 3,
        AutomationWeekdayDto::Friday => 4,
        AutomationWeekdayDto::Saturday => 5,
        AutomationWeekdayDto::Sunday => 6,
    }
}

fn to_chrono_weekday(value: AutomationWeekdayDto) -> Weekday {
    match value {
        AutomationWeekdayDto::Monday => Weekday::Mon,
        AutomationWeekdayDto::Tuesday => Weekday::Tue,
        AutomationWeekdayDto::Wednesday => Weekday::Wed,
        AutomationWeekdayDto::Thursday => Weekday::Thu,
        AutomationWeekdayDto::Friday => Weekday::Fri,
        AutomationWeekdayDto::Saturday => Weekday::Sat,
        AutomationWeekdayDto::Sunday => Weekday::Sun,
    }
}

fn rrule_weekday(value: AutomationWeekdayDto) -> &'static str {
    match value {
        AutomationWeekdayDto::Monday => "MO",
        AutomationWeekdayDto::Tuesday => "TU",
        AutomationWeekdayDto::Wednesday => "WE",
        AutomationWeekdayDto::Thursday => "TH",
        AutomationWeekdayDto::Friday => "FR",
        AutomationWeekdayDto::Saturday => "SA",
        AutomationWeekdayDto::Sunday => "SU",
    }
}

fn join_numbers(values: &[u8]) -> String {
    values
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_ms(timezone: &str, year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        let timezone: Tz = timezone.parse().unwrap();
        timezone
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    fn utc_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn fixed_interval_is_anchored_in_utc_and_strictly_after_threshold() {
        let anchor = utc_ms(2024, 3, 9, 14, 0);
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Interval {
            amount: 1,
            unit: AutomationIntervalUnitDto::Days,
            anchor_at: anchor,
            timezone: "America/New_York".to_string(),
        })
        .unwrap();

        assert_eq!(schedule.rrule, "FREQ=DAILY;INTERVAL=1");
        assert_eq!(
            schedule.next_run_at_ms(anchor).unwrap(),
            anchor + MILLIS_PER_DAY
        );
        // A fixed 24-hour interval moves from 09:00 EST to 10:00 EDT across spring-forward.
        assert_eq!(
            schedule.next_run_at_ms(anchor).unwrap(),
            local_ms("America/New_York", 2024, 3, 10, 10, 0)
        );
        assert_eq!(schedule.next_run_at_ms(anchor - 1).unwrap(), anchor);
    }

    #[test]
    fn daily_wall_time_skips_a_dst_gap() {
        let anchor = local_ms("America/New_York", 2024, 3, 9, 0, 0);
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Daily {
            time_minutes: 2 * 60 + 30,
            anchor_at: anchor,
            timezone: "America/New_York".to_string(),
        })
        .unwrap();

        let after = local_ms("America/New_York", 2024, 3, 9, 3, 0);
        assert_eq!(
            schedule.next_run_at_ms(after).unwrap(),
            local_ms("America/New_York", 2024, 3, 11, 2, 30)
        );
    }

    #[test]
    fn ambiguous_fall_back_wall_time_runs_once_at_the_earlier_instant() {
        let timezone: Tz = "America/New_York".parse().unwrap();
        let anchor = local_ms("America/New_York", 2024, 11, 2, 0, 0);
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Daily {
            time_minutes: 90,
            anchor_at: anchor,
            timezone: timezone.to_string(),
        })
        .unwrap();
        let fold = NaiveDate::from_ymd_opt(2024, 11, 3)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();
        let LocalResult::Ambiguous(first, second) = timezone.from_local_datetime(&fold) else {
            panic!("expected a DST fold");
        };
        let earlier = first.timestamp_millis().min(second.timestamp_millis());
        let later = first.timestamp_millis().max(second.timestamp_millis());

        let after_anchor_day = local_ms("America/New_York", 2024, 11, 2, 2, 0);
        assert_eq!(schedule.next_run_at_ms(after_anchor_day).unwrap(), earlier);
        assert_eq!(
            schedule.next_run_at_ms(earlier).unwrap(),
            local_ms("America/New_York", 2024, 11, 4, 1, 30)
        );
        assert_ne!(schedule.next_run_at_ms(earlier).unwrap(), later);
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        let anchor = local_ms("Asia/Shanghai", 2026, 8, 21, 8, 0); // Friday
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Weekdays {
            time_minutes: 9 * 60,
            anchor_at: anchor,
            timezone: "Asia/Shanghai".to_string(),
        })
        .unwrap();

        assert_eq!(
            schedule
                .next_run_at_ms(local_ms("Asia/Shanghai", 2026, 8, 21, 10, 0))
                .unwrap(),
            local_ms("Asia/Shanghai", 2026, 8, 24, 9, 0)
        );
        assert_eq!(
            schedule.rrule,
            "FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0;BYSECOND=0;WKST=MO"
        );
    }

    #[test]
    fn weekly_selection_is_sorted_deduplicated_and_uses_anchor_weeks() {
        let anchor = local_ms("Asia/Shanghai", 2026, 8, 17, 0, 0); // Monday
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Weekly,
            interval: 2,
            minute_of_hour: None,
            time_minutes: Some(16 * 60),
            weekdays: Some(vec![
                AutomationWeekdayDto::Friday,
                AutomationWeekdayDto::Monday,
                AutomationWeekdayDto::Friday,
            ]),
            month_days: None,
            months: None,
            anchor_at: anchor,
            timezone: "Asia/Shanghai".to_string(),
        })
        .unwrap();

        assert_eq!(
            schedule.rrule,
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,FR;BYHOUR=16;BYMINUTE=0;BYSECOND=0;WKST=MO"
        );
        assert_eq!(
            schedule
                .next_run_at_ms(local_ms("Asia/Shanghai", 2026, 8, 21, 17, 0))
                .unwrap(),
            local_ms("Asia/Shanghai", 2026, 8, 31, 16, 0)
        );
    }

    #[test]
    fn custom_daily_interval_uses_anchor_local_date() {
        let anchor = local_ms("Asia/Shanghai", 2026, 8, 23, 12, 0);
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Daily,
            interval: 3,
            minute_of_hour: None,
            time_minutes: Some(9 * 60 + 15),
            weekdays: None,
            month_days: None,
            months: None,
            anchor_at: anchor,
            timezone: "Asia/Shanghai".to_string(),
        })
        .unwrap();

        // The anchor-day 09:15 occurrence predates the 12:00 anchor, so the first valid cadence
        // slot is three local calendar days later.
        assert_eq!(
            schedule.next_run_at_ms(anchor).unwrap(),
            local_ms("Asia/Shanghai", 2026, 8, 26, 9, 15)
        );
        assert_eq!(
            schedule.rrule,
            "FREQ=DAILY;INTERVAL=3;BYHOUR=9;BYMINUTE=15;BYSECOND=0"
        );
    }

    #[test]
    fn monthly_day_31_skips_short_months() {
        let anchor = local_ms("Asia/Shanghai", 2026, 1, 1, 0, 0);
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Monthly,
            interval: 1,
            minute_of_hour: None,
            time_minutes: Some(9 * 60),
            weekdays: None,
            month_days: Some(vec![31]),
            months: None,
            anchor_at: anchor,
            timezone: "Asia/Shanghai".to_string(),
        })
        .unwrap();

        assert_eq!(
            schedule
                .next_run_at_ms(local_ms("Asia/Shanghai", 2026, 1, 31, 9, 0))
                .unwrap(),
            local_ms("Asia/Shanghai", 2026, 3, 31, 9, 0)
        );
    }

    #[test]
    fn yearly_february_29_skips_non_leap_years() {
        let anchor = local_ms("Asia/Shanghai", 2024, 1, 1, 0, 0);
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Yearly,
            interval: 1,
            minute_of_hour: None,
            time_minutes: Some(9 * 60),
            weekdays: None,
            month_days: Some(vec![29]),
            months: Some(vec![2]),
            anchor_at: anchor,
            timezone: "Asia/Shanghai".to_string(),
        })
        .unwrap();

        assert_eq!(
            schedule
                .next_run_at_ms(local_ms("Asia/Shanghai", 2024, 2, 29, 9, 0))
                .unwrap(),
            local_ms("Asia/Shanghai", 2028, 2, 29, 9, 0)
        );
    }

    #[test]
    fn custom_hourly_uses_wall_clock_minute_and_skips_fold_duplicate() {
        let anchor = local_ms("America/New_York", 2024, 11, 2, 0, 0);
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Hourly,
            interval: 1,
            minute_of_hour: Some(30),
            time_minutes: None,
            weekdays: None,
            month_days: None,
            months: None,
            anchor_at: anchor,
            timezone: "America/New_York".to_string(),
        })
        .unwrap();
        let first_fold = schedule
            .next_run_at_ms(local_ms("America/New_York", 2024, 11, 3, 0, 45))
            .unwrap();
        assert_eq!(
            schedule.next_run_at_ms(first_fold).unwrap(),
            local_ms("America/New_York", 2024, 11, 3, 2, 30)
        );
    }

    #[test]
    fn normalization_rejects_invalid_and_ambiguous_custom_fields() {
        let anchor = utc_ms(2026, 8, 23, 0, 0);
        let invalid_zone = normalize_schedule(&AutomationScheduleInputDto::Daily {
            time_minutes: 0,
            anchor_at: anchor,
            timezone: "Shanghai".to_string(),
        })
        .unwrap_err();
        assert_eq!(
            invalid_zone.code,
            AutomationScheduleErrorCode::InvalidTimezone
        );

        let zero_interval = normalize_schedule(&AutomationScheduleInputDto::Interval {
            amount: 0,
            unit: AutomationIntervalUnitDto::Minutes,
            anchor_at: anchor,
            timezone: "UTC".to_string(),
        })
        .unwrap_err();
        assert_eq!(
            zero_interval.code,
            AutomationScheduleErrorCode::InvalidInterval
        );

        let invalid_time = normalize_schedule(&AutomationScheduleInputDto::Daily {
            time_minutes: 24 * 60,
            anchor_at: anchor,
            timezone: "UTC".to_string(),
        })
        .unwrap_err();
        assert_eq!(invalid_time.code, AutomationScheduleErrorCode::InvalidTime);

        let unexpected = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Hourly,
            interval: 1,
            minute_of_hour: Some(30),
            time_minutes: Some(9 * 60),
            weekdays: None,
            month_days: None,
            months: None,
            anchor_at: anchor,
            timezone: "UTC".to_string(),
        })
        .unwrap_err();
        assert_eq!(
            unexpected.code,
            AutomationScheduleErrorCode::UnexpectedField
        );

        let missing_weekdays = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Weekly,
            interval: 1,
            minute_of_hour: None,
            time_minutes: Some(9 * 60),
            weekdays: None,
            month_days: None,
            months: None,
            anchor_at: anchor,
            timezone: "UTC".to_string(),
        })
        .unwrap_err();
        assert_eq!(
            missing_weekdays.code,
            AutomationScheduleErrorCode::MissingField
        );

        let empty_month_days = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Monthly,
            interval: 1,
            minute_of_hour: None,
            time_minutes: Some(9 * 60),
            weekdays: None,
            month_days: Some(Vec::new()),
            months: None,
            anchor_at: anchor,
            timezone: "UTC".to_string(),
        })
        .unwrap_err();
        assert_eq!(
            empty_month_days.code,
            AutomationScheduleErrorCode::EmptySelection
        );

        let invalid_month = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Yearly,
            interval: 1,
            minute_of_hour: None,
            time_minutes: Some(9 * 60),
            weekdays: None,
            month_days: Some(vec![1]),
            months: Some(vec![13]),
            anchor_at: anchor,
            timezone: "UTC".to_string(),
        })
        .unwrap_err();
        assert_eq!(
            invalid_month.code,
            AutomationScheduleErrorCode::InvalidMonth
        );

        let invalid_hour_minute = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Hourly,
            interval: 1,
            minute_of_hour: Some(60),
            time_minutes: None,
            weekdays: None,
            month_days: None,
            months: None,
            anchor_at: anchor,
            timezone: "UTC".to_string(),
        })
        .unwrap_err();
        assert_eq!(
            invalid_hour_minute.code,
            AutomationScheduleErrorCode::InvalidTime
        );
    }

    #[test]
    fn normalization_sorts_months_and_days_and_builds_canonical_rrule() {
        let anchor = utc_ms(2026, 8, 23, 0, 0);
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Yearly,
            interval: 2,
            minute_of_hour: None,
            time_minutes: Some(9 * 60 + 15),
            weekdays: None,
            month_days: Some(vec![12, 1, 12]),
            months: Some(vec![12, 1, 1]),
            anchor_at: anchor,
            timezone: "Asia/Shanghai".to_string(),
        })
        .unwrap();

        assert_eq!(
            schedule.rrule,
            "FREQ=YEARLY;INTERVAL=2;BYMONTH=1,12;BYMONTHDAY=1,12;BYHOUR=9;BYMINUTE=15;BYSECOND=0"
        );
        let AutomationScheduleInputDto::Custom {
            months, month_days, ..
        } = schedule.schedule
        else {
            panic!("expected custom schedule");
        };
        assert_eq!(months, Some(vec![1, 12]));
        assert_eq!(month_days, Some(vec![1, 12]));
    }

    #[test]
    fn schedule_with_no_possible_calendar_date_fails_closed() {
        let anchor = utc_ms(2026, 2, 1, 0, 0);
        let schedule = normalize_schedule(&AutomationScheduleInputDto::Custom {
            frequency: AutomationCustomFrequencyDto::Yearly,
            interval: 1,
            minute_of_hour: None,
            time_minutes: Some(9 * 60),
            weekdays: None,
            month_days: Some(vec![30]),
            months: Some(vec![2]),
            anchor_at: anchor,
            timezone: "UTC".to_string(),
        })
        .unwrap();

        assert_eq!(
            schedule.next_run_at_ms(anchor).unwrap_err().code,
            AutomationScheduleErrorCode::NoFutureOccurrence
        );
    }
}

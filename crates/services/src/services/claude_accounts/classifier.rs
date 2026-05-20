//! Failure classification for Claude executor exits.
//!
//! Ordering (per research.md §4): UsageExhausted → NeedsReauth → Transient → Fatal.
//! Biased toward Transient for ambiguous cases (e.g. naked 429 with no usage-window
//! context) to avoid false rotations on a stray "rate limit" mention.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use once_cell::sync::Lazy;
use regex::Regex;

use super::types::{ClaudeAccountThrottleReason, FailureClass};

/// Canonical usage-limit signal emitted by `@anthropic-ai/claude-code` when the
/// 5h or weekly subscription cap is hit. Examples:
///   `Claude AI usage limit reached|1759770000`
///   `Claude AI usage limit reached, please try again after 5:20pm`
static USAGE_LIMIT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)Claude AI usage limit reached(?:\|(\d+))?").expect("valid regex")
});

/// Permanent auth failures from the OAuth refresh path or the CLI's own
/// auth checks.
static NEEDS_REAUTH_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\binvalid_grant\b|\binvalid_token\b|token (?:has been )?revoked|unauthorized: ",
    )
    .expect("valid regex")
});

/// Transient signals — explicit rate-limit / transport failures that should
/// retry the SAME account with exponential back-off.
static TRANSIENT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        # CLI's own canonical transient-rate-limit message
        Server\ is\ temporarily\ limiting\ requests
        # generic API error wrapper
        | API\ Error
        # explicit rate-limit text NOT preceded by 'usage'
        | (?-i)rate\s*limit (?i)
        # transport errors
        | ECONNRESET | ETIMEDOUT | ENETUNREACH | fetch\ failed
        # 5xx server errors
        | \b5\d{2}\b
        ",
    )
    .expect("valid regex")
});

/// Classify a Claude executor failure based on the combined stdout/stderr text
/// and the process exit code.
///
/// Note: the input text MUST come from the CLI itself (stderr, or stdout in
/// `-p` non-interactive mode). Do NOT include user-supplied prompts or model
/// assistant content — a prompt containing the phrase "rate limit" must not
/// trigger rotation.
pub fn classify_failure(stdout: &str, stderr: &str, exit_code: Option<i32>) -> FailureClass {
    let combined = if stdout.is_empty() {
        stderr.to_string()
    } else if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stderr}\n{stdout}")
    };

    // 1. UsageExhausted is the most specific signal — check first.
    if USAGE_LIMIT_RE.is_match(&combined) {
        return FailureClass::UsageExhausted;
    }
    // 2. Permanent auth failures.
    if NEEDS_REAUTH_RE.is_match(&combined) {
        return FailureClass::NeedsReauth;
    }
    // 3. Transient — explicit rate-limit / transport / 5xx.
    if TRANSIENT_RE.is_match(&combined) {
        return FailureClass::Transient;
    }
    // 4. Bias toward Transient when the exit code suggests recoverable failure
    //    AND we have at least *some* error context — keeps recoverable runs
    //    alive on unmatched-but-likely-network failures (e.g. SIGPIPE, DNS
    //    blip with no recognized text), without classifying a clean exit 0
    //    as a failure or retrying on a hard crash.
    match exit_code {
        // Exit 0 = process exited cleanly; caller is in the wrong path.
        Some(0) => FailureClass::Fatal,
        // Negative exit codes (Unix `< 0` from `child.wait`) usually mean the
        // process was killed by a signal; treat as Fatal to surface the cause.
        Some(c) if c < 0 => FailureClass::Fatal,
        // Unknown exit code (process killed externally without status) —
        // assume transient and let the user retry budget catch a loop.
        None => FailureClass::Transient,
        // Any other non-zero exit with non-empty captured output — bias
        // toward Transient so we don't drop the attempt on an unrecognized
        // network/transient error.
        Some(_) if !combined.trim().is_empty() => FailureClass::Transient,
        // Non-zero exit with NO captured output: no signal at all — Fatal.
        Some(_) => FailureClass::Fatal,
    }
}

/// Parse the optional `|<unix_ts>` tail of the `Claude AI usage limit reached` signal.
/// Returns the reset time and whether it appears to be the 5h or weekly cap.
///
/// Heuristic: if the reset is more than 6 hours away, classify as weekly; otherwise
/// 5h. If no timestamp is present, return None (caller uses conservative defaults).
pub fn parse_usage_reset(
    text: &str,
    now: DateTime<Utc>,
) -> Option<(DateTime<Utc>, ClaudeAccountThrottleReason)> {
    let caps = USAGE_LIMIT_RE.captures(text)?;
    let ts_str = caps.get(1)?.as_str();
    let ts: i64 = ts_str.parse().ok()?;
    let reset_at = DateTime::<Utc>::from_timestamp(ts, 0)?;
    let reason = if reset_at - now > ChronoDuration::hours(6) {
        ClaudeAccountThrottleReason::Weekly
    } else {
        ClaudeAccountThrottleReason::FiveHour
    };
    Some((reset_at, reason))
}

/// Conservative fallback reset times when the CLI doesn't surface a structured
/// timestamp (per spec FR-017 / edge case "Unknown usage window").
///
/// These are intentionally pessimistic — the rotator will use the real reset
/// timestamp on the NEXT successful spawn that surfaces one, so getting the
/// fallback wrong only matters for the brief window between cap-hit and the
/// next status update.
pub fn fallback_reset(reason: ClaudeAccountThrottleReason, now: DateTime<Utc>) -> DateTime<Utc> {
    match reason {
        ClaudeAccountThrottleReason::FiveHour => now + ChronoDuration::hours(1),
        ClaudeAccountThrottleReason::Weekly => {
            // Next Monday 00:00 UTC. Anthropic's weekly cap aligns with ISO
            // weeks (Monday–Sunday). chrono's `Weekday::Mon.num_days_from_monday()`
            // is 0, so we compute "days until next Monday" as
            //   ((7 - now_weekday) % 7), with a guard for already-Monday
            //   (return next Monday, not today).
            use chrono::{Datelike, Timelike};
            let now_weekday = now.weekday().num_days_from_monday() as i64;
            let mut days_until_next_monday = (7 - now_weekday) % 7;
            if days_until_next_monday == 0 {
                days_until_next_monday = 7;
            }
            let next_monday_date = now.date_naive() + ChronoDuration::days(days_until_next_monday);
            next_monday_date
                .and_hms_opt(0, 0, 0)
                .and_then(|naive| naive.and_local_timezone(Utc).single())
                .unwrap_or_else(|| {
                    // Pathological fallback if date arithmetic somehow fails.
                    let _ = now.hour(); // touch Timelike import so it's not unused
                    now + ChronoDuration::days(7)
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_limit_pipe_form_classifies_as_usage_exhausted() {
        let stdout = "Claude AI usage limit reached|1759770000";
        assert_eq!(
            classify_failure(stdout, "", Some(1)),
            FailureClass::UsageExhausted
        );
    }

    #[test]
    fn usage_limit_human_form_classifies_as_usage_exhausted() {
        let stderr = "Claude AI usage limit reached, please try again after 5:20pm";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::UsageExhausted
        );
    }

    #[test]
    fn invalid_grant_classifies_as_needs_reauth() {
        let stderr = "OAuth refresh failed: invalid_grant";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::NeedsReauth
        );
    }

    #[test]
    fn revoked_token_classifies_as_needs_reauth() {
        let stderr = "Token has been revoked by the user";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::NeedsReauth
        );
    }

    #[test]
    fn transient_rate_limit_classifies_as_transient() {
        let stderr = "API Error: Server is temporarily limiting requests (not your usage limit)";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::Transient
        );
    }

    #[test]
    fn five_hundred_classifies_as_transient() {
        let stderr = "API Error: 503 Service Unavailable";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::Transient
        );
    }

    #[test]
    fn unknown_error_with_output_classifies_as_transient() {
        // Per spec FR-012 bias rule: unrecognized non-zero exit + non-empty
        // output is more likely a transient blip than a hard crash. Let the
        // retry budget catch a loop rather than dropping the attempt.
        let stderr = "Something else went wrong";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::Transient
        );
    }

    #[test]
    fn empty_output_with_nonzero_exit_classifies_as_fatal() {
        // No signal at all — don't retry blindly.
        assert_eq!(classify_failure("", "", Some(1)), FailureClass::Fatal);
    }

    #[test]
    fn signal_kill_classifies_as_fatal() {
        // Negative exit = killed by signal; surface as Fatal so the user
        // sees what crashed instead of retrying a doomed process.
        let stderr = "...";
        assert_eq!(classify_failure("", stderr, Some(-1)), FailureClass::Fatal);
    }

    #[test]
    fn usage_limit_wins_over_transient() {
        // Both signals present — UsageExhausted must win.
        let stderr = "API Error: 503 Service Unavailable\nClaude AI usage limit reached|1759770000";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::UsageExhausted
        );
    }

    #[test]
    fn needs_reauth_wins_over_transient() {
        let stderr = "API Error: 503 Service Unavailable\ninvalid_grant";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::NeedsReauth
        );
    }

    #[test]
    fn parse_usage_reset_extracts_timestamp_and_five_hour() {
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        // Reset 1 hour in the future.
        let stdout = format!("Claude AI usage limit reached|{}", 1_700_000_000 + 3600);
        let (reset, reason) = parse_usage_reset(&stdout, now).unwrap();
        assert_eq!(reset.timestamp(), 1_700_000_000 + 3600);
        assert_eq!(reason, ClaudeAccountThrottleReason::FiveHour);
    }

    #[test]
    fn parse_usage_reset_far_future_is_weekly() {
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        // Reset 3 days in the future.
        let stdout = format!(
            "Claude AI usage limit reached|{}",
            1_700_000_000 + 3 * 24 * 3600
        );
        let (_, reason) = parse_usage_reset(&stdout, now).unwrap();
        assert_eq!(reason, ClaudeAccountThrottleReason::Weekly);
    }

    #[test]
    fn parse_usage_reset_returns_none_without_timestamp() {
        let now = Utc::now();
        let stdout = "Claude AI usage limit reached, please try again after 5:20pm";
        assert!(parse_usage_reset(stdout, now).is_none());
    }

    #[test]
    fn fallback_reset_weekly_is_next_monday_midnight_utc() {
        use chrono::{Datelike, TimeZone, Timelike};
        // Pick a known Wednesday — 2026-05-13.
        let wed = Utc.with_ymd_and_hms(2026, 5, 13, 9, 30, 0).unwrap();
        assert_eq!(
            wed.weekday().num_days_from_monday(),
            2,
            "sanity: chosen date must be Wednesday"
        );
        let reset = fallback_reset(ClaudeAccountThrottleReason::Weekly, wed);
        assert_eq!(
            reset.weekday().num_days_from_monday(),
            0,
            "weekly reset must land on a Monday"
        );
        assert_eq!(reset.hour(), 0);
        assert_eq!(reset.minute(), 0);
        assert!(reset > wed, "reset must be strictly after now");
    }

    #[test]
    fn fallback_reset_weekly_when_today_is_monday_returns_next_monday() {
        use chrono::{Datelike, TimeZone};
        // A Monday — 2026-05-11.
        let mon = Utc.with_ymd_and_hms(2026, 5, 11, 12, 0, 0).unwrap();
        assert_eq!(mon.weekday().num_days_from_monday(), 0);
        let reset = fallback_reset(ClaudeAccountThrottleReason::Weekly, mon);
        // Must be the FOLLOWING Monday, not today.
        assert!(reset > mon);
        assert_eq!(reset.weekday().num_days_from_monday(), 0);
        // Mon 12:00 → next Mon 00:00 = 6.5 days; (reset - mon) is ChronoDuration,
        // and num_days() on it truncates to 6.
        let delta: ChronoDuration = reset - mon;
        assert_eq!(delta.num_days(), 6);
    }
}

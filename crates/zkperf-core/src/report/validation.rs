use std::collections::{HashMap, HashSet};

use serde_json::Number;

use crate::measurement::{ObservationView, StatisticsView};
use crate::{
    AttemptId, Availability, Measurement, Metric, PercentileMethod, Reason, SampleId, SampleStatus,
    Slug,
};

use super::{BenchmarkReportV1Parts, ReportError, ReportStatus};

#[derive(Clone, Debug, PartialEq)]
struct AttemptSignature {
    attempt_index: u64,
    schedule_position: u64,
    warmup: bool,
    started_at: String,
    finished_at: String,
    status: SampleStatus,
    replacement_attempt_id: Option<AttemptId>,
    error_code: Option<Slug>,
    reason: Option<Reason>,
}

pub(super) fn validate_report(parts: &BenchmarkReportV1Parts) -> Result<(), ReportError> {
    if parts.measurements.is_empty() {
        return invalid("report requires at least one measurement");
    }

    let schedule = validate_schedule(parts)?;
    let mut measurement_keys = HashSet::new();
    let mut sample_ids = HashSet::<SampleId>::new();
    let mut attempts = HashMap::<AttemptId, AttemptSignature>::new();
    let mut proof_required = HashSet::<(String, AttemptId)>::new();
    let mut proof_sizes = HashMap::<(String, AttemptId), usize>::new();

    for (measurement_index, measurement) in parts.measurements.iter().enumerate() {
        measurement
            .validate_correctness(&parts.benchmark)
            .map_err(|error| {
                ReportError::InvalidGraph(format!("/measurements/{measurement_index}: {error}"))
            })?;

        let phase_key = phase_key(measurement)?;
        if !measurement_keys.insert((measurement.metric(), phase_key.clone())) {
            return invalid(format!(
                "/measurements/{measurement_index}: duplicate metric and phase"
            ));
        }
        if measurement.availability() == Availability::Unavailable {
            continue;
        }

        let observations = measurement.observations();
        let mut measurement_attempts = HashSet::new();
        let mut measurement_schedule_positions = HashSet::new();
        for (sample_index, observation) in observations.iter().copied().enumerate() {
            let sample_path = format!("/measurements/{measurement_index}/samples/{sample_index}");
            if !sample_ids.insert(observation.identity.id()) {
                return invalid(format!("{sample_path}/id: duplicate sample ID"));
            }
            if !measurement_attempts.insert(observation.identity.attempt_id()) {
                return invalid(format!(
                    "{sample_path}/attempt_id: duplicate attempt in measurement"
                ));
            }
            if !measurement_schedule_positions.insert(observation.identity.schedule_position()) {
                return invalid(format!(
                    "{sample_path}/schedule_position: duplicate schedule position in measurement"
                ));
            }

            validate_schedule_reference(parts, measurement, observation, &schedule, &sample_path)?;

            let signature = attempt_signature(observation);
            match attempts.get(&observation.identity.attempt_id()) {
                Some(previous) if previous != &signature => {
                    return invalid(format!(
                        "{sample_path}: attempt metadata differs across metrics"
                    ));
                }
                Some(_) => {}
                None => {
                    attempts.insert(observation.identity.attempt_id(), signature);
                }
            }

            if measurement.metric() == Metric::Duration
                && observation.status == SampleStatus::Success
                && measurement
                    .phase()
                    .is_some_and(crate::Phase::is_proof_related)
            {
                proof_required.insert((phase_key.clone(), observation.identity.attempt_id()));
            }
            if measurement.metric() == Metric::ProofSize
                && observation.status == SampleStatus::Success
            {
                *proof_sizes
                    .entry((phase_key.clone(), observation.identity.attempt_id()))
                    .or_default() += 1;
            }
        }

        validate_statistics(parts, measurement_index, measurement, &observations)?;
    }

    validate_replacements(&attempts)?;
    validate_proof_sizes(&proof_required, &proof_sizes)?;
    validate_attempt_references(parts, &attempts)?;
    validate_report_status(parts, &attempts)
}

fn validate_schedule(
    parts: &BenchmarkReportV1Parts,
) -> Result<HashMap<u64, &super::BenchmarkJob>, ReportError> {
    let mut schedule = HashMap::new();
    for entry in &parts.run.planned_order {
        if schedule.insert(entry.position, entry).is_some() {
            return invalid(format!(
                "/run/planned_order: duplicate position {}",
                entry.position
            ));
        }
    }
    Ok(schedule)
}

fn validate_schedule_reference(
    parts: &BenchmarkReportV1Parts,
    measurement: &Measurement,
    observation: ObservationView<'_>,
    schedule: &HashMap<u64, &super::BenchmarkJob>,
    path: &str,
) -> Result<(), ReportError> {
    let Some(planned) = schedule.get(&observation.identity.schedule_position()) else {
        return invalid(format!("{path}/schedule_position: unknown schedule entry"));
    };
    if &planned.engine_id != parts.engine.id() {
        return invalid(format!("{path}: engine does not match schedule"));
    }
    if measurement
        .phase()
        .is_some_and(|phase| phase != &planned.phase)
    {
        return invalid(format!("{path}: phase does not match schedule"));
    }
    if planned.attempt_index != observation.identity.attempt_index() {
        return invalid(format!("{path}: attempt index does not match schedule"));
    }
    if planned.warmup != observation.identity.is_warmup() {
        return invalid(format!("{path}: warm-up flag does not match schedule"));
    }
    Ok(())
}

fn validate_statistics(
    parts: &BenchmarkReportV1Parts,
    measurement_index: usize,
    measurement: &Measurement,
    observations: &[ObservationView<'_>],
) -> Result<(), ReportError> {
    let path = format!("/measurements/{measurement_index}/statistics");
    let statistics = measurement
        .statistics()
        .ok_or_else(|| ReportError::InvalidGraph(format!("{path}: missing statistics")))?;
    validate_counts_and_rates(&path, &statistics, observations)?;
    validate_duration_policy(measurement_index, measurement, observations)?;
    validate_statistics_population(parts, &path, statistics, observations)
}

fn validate_counts_and_rates(
    path: &str,
    statistics: &StatisticsView<'_>,
    observations: &[ObservationView<'_>],
) -> Result<(), ReportError> {
    let counts = observed_status_counts(observations);
    let measured: Vec<_> = observations
        .iter()
        .copied()
        .filter(|observation| !observation.identity.is_warmup())
        .collect();
    let denominator = count_as_f64(measured.len());
    let expected_failure_rate = if measured.is_empty() {
        0.0
    } else {
        count_as_f64(
            measured
                .iter()
                .filter(|observation| observation.status == SampleStatus::Failed)
                .count(),
        ) / denominator
    };
    let expected_timeout_rate = if measured.is_empty() {
        0.0
    } else {
        count_as_f64(
            measured
                .iter()
                .filter(|observation| observation.status == SampleStatus::TimedOut)
                .count(),
        ) / denominator
    };

    let (status_counts, failure_rate, timeout_rate) = match statistics {
        StatisticsView::Available {
            status_counts,
            failure_rate,
            timeout_rate,
            ..
        }
        | StatisticsView::Unavailable {
            status_counts,
            failure_rate,
            timeout_rate,
        } => (*status_counts, *failure_rate, *timeout_rate),
    };
    if !status_counts_match(status_counts, counts) {
        return invalid(format!("{path}/status_counts: count mismatch"));
    }
    if !float_eq(failure_rate, expected_failure_rate) {
        return invalid(format!("{path}/rates/failure: rate mismatch"));
    }
    if !float_eq(timeout_rate, expected_timeout_rate) {
        return invalid(format!("{path}/rates/timeout: rate mismatch"));
    }
    Ok(())
}

fn validate_duration_policy(
    measurement_index: usize,
    measurement: &Measurement,
    observations: &[ObservationView<'_>],
) -> Result<(), ReportError> {
    if let Some((expected_warmups, expected_measured)) = measurement.duration_policy_counts() {
        let warmups = count_as_u64(
            observations
                .iter()
                .filter(|observation| observation.identity.is_warmup())
                .count(),
        )?;
        let planned_measured = count_as_u64(
            observations
                .iter()
                .filter(|observation| {
                    !observation.identity.is_warmup()
                        && observation.identity.replacement_attempt_id().is_none()
                })
                .count(),
        )?;
        if warmups != expected_warmups {
            return invalid(format!(
                "/measurements/{measurement_index}/policy/warmup_count: count mismatch"
            ));
        }
        if planned_measured != expected_measured {
            return invalid(format!(
                "/measurements/{measurement_index}/policy/measured_trial_count: count mismatch"
            ));
        }
    }
    Ok(())
}

fn validate_statistics_population(
    parts: &BenchmarkReportV1Parts,
    path: &str,
    statistics: StatisticsView<'_>,
    observations: &[ObservationView<'_>],
) -> Result<(), ReportError> {
    let eligible_ids: HashSet<_> = observations
        .iter()
        .filter(|observation| {
            observation.status == SampleStatus::Success && !observation.identity.is_warmup()
        })
        .map(|observation| observation.identity.attempt_id())
        .collect();
    match statistics {
        StatisticsView::Unavailable { .. } => {
            if eligible_ids.is_empty() {
                Ok(())
            } else {
                invalid(format!(
                    "{path}: unavailable statistics have eligible successful samples"
                ))
            }
        }
        StatisticsView::Available {
            sample_count,
            minimum,
            maximum,
            median,
            mean,
            standard_deviation,
            percentiles,
            included_attempt_ids,
            excluded_warmup_attempt_ids,
            flagged_outlier_attempt_ids,
            ..
        } => {
            let included_ids: HashSet<_> = included_attempt_ids.iter().copied().collect();
            if included_ids != eligible_ids {
                return invalid(format!(
                    "{path}/included_attempt_ids: eligible attempt set mismatch"
                ));
            }
            if sample_count != count_as_u64(included_attempt_ids.len())? {
                return invalid(format!("{path}/sample_count: count mismatch"));
            }

            validate_attempt_sets(
                path,
                observations,
                &included_ids,
                excluded_warmup_attempt_ids,
                flagged_outlier_attempt_ids,
            )?;

            let samples_by_attempt: HashMap<_, _> = observations
                .iter()
                .map(|observation| (observation.identity.attempt_id(), *observation))
                .collect();
            let values: Vec<_> = included_attempt_ids
                .iter()
                .map(|attempt_id| {
                    samples_by_attempt
                        .get(attempt_id)
                        .and_then(|observation| observation.value)
                        .ok_or_else(|| {
                            ReportError::InvalidGraph(format!(
                                "{path}/included_attempt_ids: unknown or valueless attempt"
                            ))
                        })
                })
                .collect::<Result<_, _>>()?;
            validate_summary(
                path,
                &values,
                minimum,
                maximum,
                median,
                mean,
                standard_deviation,
                &percentiles,
                parts.run.policy.percentile_method(),
            )
        }
    }
}

fn validate_attempt_sets(
    path: &str,
    observations: &[ObservationView<'_>],
    included_ids: &HashSet<AttemptId>,
    excluded_warmup_attempt_ids: &[AttemptId],
    flagged_outlier_attempt_ids: &[AttemptId],
) -> Result<(), ReportError> {
    let warmup_ids: HashSet<_> = observations
        .iter()
        .filter(|observation| observation.identity.is_warmup())
        .map(|observation| observation.identity.attempt_id())
        .collect();
    if excluded_warmup_attempt_ids
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        != warmup_ids
    {
        return invalid(format!(
            "{path}/excluded_warmup_attempt_ids: warm-up set mismatch"
        ));
    }
    if !flagged_outlier_attempt_ids
        .iter()
        .all(|attempt_id| included_ids.contains(attempt_id))
    {
        return invalid(format!(
            "{path}/flagged_outlier_attempt_ids: outlier not included"
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_summary(
    path: &str,
    values: &[u64],
    minimum: &Number,
    maximum: &Number,
    median: &Number,
    mean: Option<&Number>,
    standard_deviation: Option<&Number>,
    percentiles: &[(&str, &Number)],
    percentile_method: PercentileMethod,
) -> Result<(), ReportError> {
    if values.is_empty() {
        return invalid(format!("{path}: available statistics have no values"));
    }
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    let expected_minimum = ordered[0];
    let expected_maximum = ordered[ordered.len() - 1];
    let expected_median_twice = if ordered.len() % 2 == 0 {
        u128::from(ordered[ordered.len() / 2 - 1]) + u128::from(ordered[ordered.len() / 2])
    } else {
        u128::from(ordered[ordered.len() / 2]) * 2
    };
    let expected_sum = ordered.iter().copied().map(u128::from).sum();
    let float_values: Vec<_> = ordered.iter().copied().map(value_as_f64).collect();
    let expected_mean = ratio_as_f64(expected_sum, ordered.len());
    let expected_deviation = if ordered.len() == 1 {
        0.0
    } else {
        let squared_error = float_values
            .iter()
            .map(|value| (value - expected_mean).powi(2))
            .sum::<f64>();
        (squared_error / count_as_f64(ordered.len() - 1)).sqrt()
    };

    if !number_matches_twice(minimum, u128::from(expected_minimum) * 2) {
        return invalid(format!("{path}/minimum: summary mismatch"));
    }
    if !number_matches_twice(maximum, u128::from(expected_maximum) * 2) {
        return invalid(format!("{path}/maximum: summary mismatch"));
    }
    if !number_matches_twice(median, expected_median_twice) {
        return invalid(format!("{path}/median: summary mismatch"));
    }
    if mean.is_some_and(|actual| {
        !number_matches_mean(actual, expected_sum, ordered.len(), expected_mean)
    }) {
        return invalid(format!("{path}/mean: summary mismatch"));
    }
    if standard_deviation.is_some_and(|actual| !number_float_eq(actual, expected_deviation)) {
        return invalid(format!("{path}/standard_deviation: summary mismatch"));
    }
    for (name, actual) in percentiles {
        let percentile = name[1..].parse::<f64>().map_err(|_| {
            ReportError::InvalidGraph(format!("{path}/percentiles/{name}: invalid"))
        })?;
        let expected = percentile_value(&ordered, percentile / 100.0, percentile_method);
        let matches = match expected {
            ExpectedPercentile::ExactTwice(value) => number_matches_twice(actual, value),
            ExpectedPercentile::Approximate(value) => number_float_eq(actual, value),
        };
        if !matches {
            return invalid(format!("{path}/percentiles/{name}: percentile mismatch"));
        }
    }
    Ok(())
}

enum ExpectedPercentile {
    ExactTwice(u128),
    Approximate(f64),
}

fn percentile_value(
    values: &[u64],
    percentile: f64,
    method: PercentileMethod,
) -> ExpectedPercentile {
    if method == PercentileMethod::NearestRank {
        let rank = position_as_index((percentile * count_as_f64(values.len())).ceil().max(1.0));
        return ExpectedPercentile::ExactTwice(
            u128::from(values[(rank - 1).min(values.len() - 1)]) * 2,
        );
    }
    let position = percentile * count_as_f64(values.len() - 1);
    let lower = position_as_index(position.floor());
    let upper = position_as_index(position.ceil());
    match method {
        PercentileMethod::Lower => ExpectedPercentile::ExactTwice(u128::from(values[lower]) * 2),
        PercentileMethod::Higher => ExpectedPercentile::ExactTwice(u128::from(values[upper]) * 2),
        PercentileMethod::Midpoint => {
            ExpectedPercentile::ExactTwice(u128::from(values[lower]) + u128::from(values[upper]))
        }
        PercentileMethod::Linear | PercentileMethod::NearestRank if lower == upper => {
            ExpectedPercentile::ExactTwice(u128::from(values[lower]) * 2)
        }
        PercentileMethod::Linear | PercentileMethod::NearestRank => {
            let lower_value = value_as_f64(values[lower]);
            let upper_value = value_as_f64(values[upper]);
            ExpectedPercentile::Approximate(
                lower_value + (upper_value - lower_value) * (position - count_as_f64(lower)),
            )
        }
    }
}

fn number_matches_twice(actual: &Number, expected_twice: u128) -> bool {
    number_matches_ratio(actual, expected_twice, 2)
}

fn number_matches_mean(
    actual: &Number,
    expected_sum: u128,
    count: usize,
    expected_float: f64,
) -> bool {
    let Some(count) = u128::try_from(count).ok() else {
        return false;
    };
    if number_matches_ratio(actual, expected_sum, count) {
        return true;
    }
    if ratio_has_terminating_decimal(expected_sum, count) {
        return false;
    }
    actual
        .as_f64()
        .is_some_and(|actual| actual.to_bits() == expected_float.to_bits())
}

fn number_matches_ratio(actual: &Number, expected_numerator: u128, denominator: u128) -> bool {
    let text = actual.to_string();
    let (negative, unsigned) = text
        .strip_prefix('-')
        .map_or((false, text.as_str()), |unsigned| (true, unsigned));
    let (significand, exponent) = match unsigned.find(['e', 'E']) {
        Some(index) => (
            &unsigned[..index],
            unsigned[index + 1..].parse::<i32>().ok(),
        ),
        None => (unsigned, Some(0)),
    };
    let Some(exponent) = exponent else {
        return false;
    };
    let fraction_digits = significand
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let raw_digits = significand.replace('.', "");
    let Some(first_nonzero) = raw_digits.bytes().position(|digit| digit != b'0') else {
        return expected_numerator == 0;
    };
    if negative {
        return false;
    }
    let trailing_zeros = raw_digits
        .bytes()
        .rev()
        .take_while(|digit| *digit == b'0')
        .count();
    let significant_end = raw_digits.len() - trailing_zeros;
    let Ok(fraction_digits) = i32::try_from(fraction_digits) else {
        return false;
    };
    let Ok(trailing_zeros) = i32::try_from(trailing_zeros) else {
        return false;
    };
    let Some(adjusted_exponent) = exponent
        .checked_sub(fraction_digits)
        .and_then(|value| value.checked_add(trailing_zeros))
    else {
        return false;
    };
    terminating_ratio_decimal(expected_numerator, denominator).is_some_and(
        |(expected_digits, expected_exponent)| {
            raw_digits[first_nonzero..significant_end] == expected_digits
                && adjusted_exponent == expected_exponent
        },
    )
}

fn ratio_has_terminating_decimal(numerator: u128, denominator: u128) -> bool {
    reduced_terminating_ratio(numerator, denominator).is_some()
}

fn terminating_ratio_decimal(numerator: u128, denominator: u128) -> Option<(String, i32)> {
    let (numerator, twos, fives) = reduced_terminating_ratio(numerator, denominator)?;
    if numerator == 0 {
        return Some((String::new(), 0));
    }
    let scale = twos.max(fives);
    let mut digits = numerator.to_string().into_bytes();
    for _ in twos..scale {
        multiply_decimal_digits(&mut digits, 2);
    }
    for _ in fives..scale {
        multiply_decimal_digits(&mut digits, 5);
    }
    let trailing_zeros = digits
        .iter()
        .rev()
        .take_while(|digit| **digit == b'0')
        .count();
    digits.truncate(digits.len() - trailing_zeros);
    let exponent = i32::try_from(trailing_zeros)
        .ok()?
        .checked_sub(i32::try_from(scale).ok()?)?;
    Some((
        String::from_utf8(digits).expect("decimal multiplication preserves ASCII"),
        exponent,
    ))
}

fn reduced_terminating_ratio(numerator: u128, denominator: u128) -> Option<(u128, u32, u32)> {
    if denominator == 0 {
        return None;
    }
    let divisor = greatest_common_divisor(numerator, denominator);
    let numerator = numerator / divisor;
    let mut denominator = denominator / divisor;
    let mut twos = 0;
    while denominator % 2 == 0 {
        denominator /= 2;
        twos += 1;
    }
    let mut fives = 0;
    while denominator % 5 == 0 {
        denominator /= 5;
        fives += 1;
    }
    (denominator == 1).then_some((numerator, twos, fives))
}

fn multiply_decimal_digits(digits: &mut Vec<u8>, factor: u8) {
    let mut carry = 0_u16;
    for digit in digits.iter_mut().rev() {
        let product = u16::from(*digit - b'0') * u16::from(factor) + carry;
        *digit = b'0' + u8::try_from(product % 10).expect("decimal digit fits in u8");
        carry = product / 10;
    }
    if carry != 0 {
        digits.insert(
            0,
            b'0' + u8::try_from(carry).expect("single-digit multiplier leaves one carry digit"),
        );
    }
}

const fn greatest_common_divisor(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn number_float_eq(actual: &Number, expected: f64) -> bool {
    actual
        .as_f64()
        .is_some_and(|actual| float_eq(actual, expected))
}

fn count_as_u64(value: usize) -> Result<u64, ReportError> {
    u64::try_from(value)
        .map_err(|_| ReportError::InvalidGraph("sample count exceeds u64".to_owned()))
}

#[allow(clippy::cast_precision_loss)]
fn count_as_f64(value: usize) -> f64 {
    value as f64
}

fn ratio_as_f64(numerator: u128, denominator: usize) -> f64 {
    const FRACTION_BITS: u32 = f64::MANTISSA_DIGITS - 1;
    const EXPONENT_BIAS: i32 = 1023;

    if numerator == 0 {
        return 0.0;
    }
    let denominator = u128::try_from(denominator).expect("usize always fits in u128");
    let numerator_bits =
        i32::try_from(u128::BITS - numerator.leading_zeros()).expect("u128 bit count fits in i32");
    let denominator_bits = i32::try_from(u128::BITS - denominator.leading_zeros())
        .expect("u128 bit count fits in i32");
    let mut exponent = numerator_bits - denominator_bits;
    let below_exponent = if exponent >= 0 {
        numerator
            < denominator
                .checked_shl(exponent.unsigned_abs())
                .expect("ratio exponent is bounded by numerator width")
    } else {
        numerator
            .checked_shl(exponent.unsigned_abs())
            .expect("ratio exponent is bounded by denominator width")
            < denominator
    };
    if below_exponent {
        exponent -= 1;
    }

    let precision_shift =
        i32::try_from(FRACTION_BITS).expect("f64 precision fits in i32") - exponent;
    let (scaled_numerator, scaled_denominator) = if precision_shift >= 0 {
        (
            numerator
                .checked_shl(precision_shift.unsigned_abs())
                .expect("u64 sample mean fits in u128 at f64 precision"),
            denominator,
        )
    } else {
        (
            numerator,
            denominator
                .checked_shl(precision_shift.unsigned_abs())
                .expect("u64 sample mean denominator fits in u128 at f64 precision"),
        )
    };
    let mut significand = scaled_numerator / scaled_denominator;
    let remainder = scaled_numerator % scaled_denominator;
    let twice_remainder = remainder
        .checked_mul(2)
        .expect("scaled denominator leaves one rounding bit");
    if twice_remainder > scaled_denominator
        || (twice_remainder == scaled_denominator && significand % 2 == 1)
    {
        significand += 1;
    }

    let carry = 1_u128 << f64::MANTISSA_DIGITS;
    if significand == carry {
        significand >>= 1;
        exponent += 1;
    }
    let implicit_bit = 1_u128 << FRACTION_BITS;
    let fraction =
        u64::try_from(significand - implicit_bit).expect("f64 fraction fits in its storage bits");
    let biased_exponent =
        u64::try_from(exponent + EXPONENT_BIAS).expect("u64 sample means are normal f64 values");
    f64::from_bits((biased_exponent << FRACTION_BITS) | fraction)
}

#[allow(clippy::cast_precision_loss)]
fn value_as_f64(value: u64) -> f64 {
    value as f64
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn position_as_index(value: f64) -> usize {
    value as usize
}

fn validate_replacements(
    attempts: &HashMap<AttemptId, AttemptSignature>,
) -> Result<(), ReportError> {
    for (attempt_id, signature) in attempts {
        let Some(target_id) = signature.replacement_attempt_id else {
            continue;
        };
        let mut lineage = HashSet::from([*attempt_id]);
        let mut current_id = target_id;
        loop {
            if !lineage.insert(current_id) {
                return invalid(format!(
                    "attempt {attempt_id}: replacement lineage contains a cycle"
                ));
            }
            let Some(target) = attempts.get(&current_id) else {
                return invalid(format!(
                    "attempt {attempt_id}: replacement target does not exist"
                ));
            };
            if target.status != SampleStatus::Invalid {
                return invalid(format!(
                    "attempt {attempt_id}: replacement target is not invalid"
                ));
            }
            let Some(parent_id) = target.replacement_attempt_id else {
                break;
            };
            current_id = parent_id;
        }
    }
    Ok(())
}

fn validate_proof_sizes(
    required: &HashSet<(String, AttemptId)>,
    observed: &HashMap<(String, AttemptId), usize>,
) -> Result<(), ReportError> {
    for key @ (phase, attempt_id) in required {
        let count = observed.get(key).copied().unwrap_or_default();
        if count != 1 {
            return invalid(format!(
                "attempt {attempt_id}: expected one proof_size observation for phase {phase}, found {count}"
            ));
        }
    }
    Ok(())
}

fn validate_attempt_references(
    parts: &BenchmarkReportV1Parts,
    attempts: &HashMap<AttemptId, AttemptSignature>,
) -> Result<(), ReportError> {
    for (index, artifact) in parts.artifacts.iter().enumerate() {
        if artifact
            .attempt_id
            .is_some_and(|attempt_id| !attempts.contains_key(&attempt_id))
        {
            return invalid(format!("/artifacts/{index}/attempt_id: unknown attempt"));
        }
    }
    for (index, warning) in parts.warnings.iter().enumerate() {
        if warning
            .attempt_id
            .is_some_and(|attempt_id| !attempts.contains_key(&attempt_id))
        {
            return invalid(format!("/warnings/{index}/attempt_id: unknown attempt"));
        }
    }
    Ok(())
}

fn validate_report_status(
    parts: &BenchmarkReportV1Parts,
    attempts: &HashMap<AttemptId, AttemptSignature>,
) -> Result<(), ReportError> {
    let has_status = |status| attempts.values().any(|attempt| attempt.status == status);
    let unavailable = parts
        .measurements
        .iter()
        .any(|measurement| measurement.availability() == Availability::Unavailable);
    let consistent = match parts.status {
        ReportStatus::Successful => {
            !unavailable
                && !has_status(SampleStatus::Unsupported)
                && !has_status(SampleStatus::Failed)
                && !has_status(SampleStatus::TimedOut)
                && !has_status(SampleStatus::Invalid)
        }
        ReportStatus::Failed { .. } => attempts.is_empty() || has_status(SampleStatus::Failed),
        ReportStatus::TimedOut { .. } => has_status(SampleStatus::TimedOut),
        ReportStatus::PartiallySupported { .. } => unavailable,
        ReportStatus::Invalid { .. } => has_status(SampleStatus::Invalid),
    };
    if consistent {
        Ok(())
    } else {
        invalid("/status/outcome: report status contradicts observations")
    }
}

fn observed_status_counts(observations: &[ObservationView<'_>]) -> [u64; 5] {
    let mut counts = [0; 5];
    for observation in observations {
        counts[status_index(observation.status)] += 1;
    }
    counts
}

fn status_counts_match(counts: &crate::StatusCounts, expected: [u64; 5]) -> bool {
    [
        counts.success,
        counts.unsupported,
        counts.failed,
        counts.timed_out,
        counts.invalid,
    ] == expected
}

const fn status_index(status: SampleStatus) -> usize {
    match status {
        SampleStatus::Success => 0,
        SampleStatus::Unsupported => 1,
        SampleStatus::Failed => 2,
        SampleStatus::TimedOut => 3,
        SampleStatus::Invalid => 4,
    }
}

fn attempt_signature(observation: ObservationView<'_>) -> AttemptSignature {
    AttemptSignature {
        attempt_index: observation.identity.attempt_index(),
        schedule_position: observation.identity.schedule_position(),
        warmup: observation.identity.is_warmup(),
        started_at: observation.identity.started_at().as_str().to_owned(),
        finished_at: observation.identity.finished_at().as_str().to_owned(),
        status: observation.status,
        replacement_attempt_id: observation.identity.replacement_attempt_id(),
        error_code: observation.error_code.cloned(),
        reason: observation.reason.cloned(),
    }
}

fn phase_key(measurement: &Measurement) -> Result<String, ReportError> {
    serde_json::to_string(&measurement.phase())
        .map_err(|error| ReportError::InvalidGraph(format!("failed to encode phase: {error}")))
}

fn float_eq(left: f64, right: f64) -> bool {
    (left - right).abs() <= f64::EPSILON.max(1e-9 * left.abs().max(right.abs()))
}

fn invalid<T>(message: impl Into<String>) -> Result<T, ReportError> {
    Err(ReportError::InvalidGraph(message.into()))
}

#[cfg(test)]
mod tests {
    use serde_json::Number;

    use super::{number_matches_ratio, ratio_as_f64};

    #[test]
    fn exact_ratio_comparison_avoids_cross_product_overflow() {
        let denominator = 10_000_000_000_u128;
        let numerator = u128::from(u64::MAX) * denominator - 1;
        let actual: Number = serde_json::from_str("18446744073709551614.9999999999").unwrap();

        assert!(number_matches_ratio(&actual, numerator, denominator));

        for encoded in ["0.5", "0.50", "5e-1"] {
            let actual: Number = serde_json::from_str(encoded).unwrap();
            assert!(number_matches_ratio(&actual, 1, 2));
        }
        let smallest_u64_fraction: Number = serde_json::from_str(
            "0.0000000000000000000542101086242752217003726400434970855712890625",
        )
        .unwrap();
        assert!(number_matches_ratio(
            &smallest_u64_fraction,
            1,
            1_u128 << 64
        ));
    }

    #[test]
    fn rational_conversion_rounds_once_to_nearest_even() {
        let midpoint_above_even = (1_u128 << f64::MANTISSA_DIGITS) + 1;
        assert_eq!(
            ratio_as_f64(midpoint_above_even, 1).to_bits(),
            0x4340_0000_0000_0000
        );

        let midpoint_above_odd = midpoint_above_even + 2;
        assert_eq!(
            ratio_as_f64(midpoint_above_odd, 1).to_bits(),
            0x4340_0000_0000_0002
        );

        let reported_sum = u128::from(u64::MAX) * 2 + u128::from(469_393_303_678_674_661_u64);
        assert_eq!(
            ratio_as_f64(reported_sum, 3).to_bits(),
            0x43e5_9ad1_47b5_924a
        );
        assert_eq!(ratio_as_f64(1, 3).to_bits(), (1.0_f64 / 3.0).to_bits());
    }
}

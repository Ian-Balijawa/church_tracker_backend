//! Pure, deterministic contribution accounting. All amounts are whole UGX.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;
use uuid::Uuid;

pub const MAX_MONEY: i64 = 9_007_199_254_740_991;
const MAX_PERIODS: usize = 100_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AccountingError {
    #[error("amount must be a positive safe integer")]
    InvalidAmount,
    #[error("invalid pledge dates or version history")]
    InvalidPledge,
    #[error("ledger exceeds safe integer range")]
    Overflow,
    #[error("ledger contains too many periods")]
    TooManyPeriods,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AmountVersion {
    pub id: Uuid,
    pub amount: i64,
    pub effective_from: NaiveDate,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pledge {
    pub id: Uuid,
    pub project_id: Uuid,
    pub frequency: Frequency,
    pub start_date: NaiveDate,
    pub end_date: Option<NaiveDate>,
    pub versions: Vec<AmountVersion>,
    #[serde(default)]
    pub events: Vec<StateEvent>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StateAction {
    Pause,
    Resume,
    Cancel,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateEvent {
    pub action: StateAction,
    pub effective_date: NaiveDate,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payment {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub amount: i64,
    pub date: NaiveDate,
    pub voided: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Allocation {
    pub payment_id: Uuid,
    pub pledge_id: Uuid,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub amount: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    pub demand: i64,
    pub paid: i64,
    pub debt: i64,
    pub due_today: i64,
    pub advance: i64,
    pub unallocated_credit: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Replay {
    pub allocations: Vec<Allocation>,
    pub payment_credit: BTreeMap<Uuid, i64>,
    pub pledge_balances: BTreeMap<Uuid, Balance>,
    pub balance: Balance,
}

#[derive(Clone)]
struct Period {
    pledge_id: Uuid,
    project_id: Uuid,
    start: NaiveDate,
    end: NaiveDate,
    amount: i64,
    paid: i64,
}

fn add(value: &mut i64, amount: i64) -> Result<(), AccountingError> {
    *value = value
        .checked_add(amount)
        .filter(|sum| *sum <= MAX_MONEY)
        .ok_or(AccountingError::Overflow)?;
    Ok(())
}

fn period_end(start: NaiveDate, frequency: Frequency) -> Result<NaiveDate, AccountingError> {
    match frequency {
        Frequency::Daily => Ok(start),
        Frequency::Weekly => start
            .checked_add_signed(Duration::days(i64::from(
                Weekday::Sun.num_days_from_monday() - start.weekday().num_days_from_monday(),
            )))
            .ok_or(AccountingError::Overflow),
        Frequency::Monthly => {
            let first_next = if start.month() == 12 {
                NaiveDate::from_ymd_opt(start.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
            }
            .ok_or(AccountingError::Overflow)?;
            first_next.pred_opt().ok_or(AccountingError::Overflow)
        }
    }
}

fn periods_for(
    pledge: &Pledge,
    horizon: NaiveDate,
    remaining_budget: &mut usize,
) -> Result<Vec<Period>, AccountingError> {
    if pledge.end_date.is_some_and(|end| end < pledge.start_date) || pledge.versions.is_empty() {
        return Err(AccountingError::InvalidPledge);
    }
    let mut versions = pledge.versions.clone();
    versions.sort_by_key(|version| version.effective_from);
    if versions[0].effective_from != pledge.start_date
        || versions
            .iter()
            .any(|version| version.amount <= 0 || version.amount > MAX_MONEY)
    {
        return Err(AccountingError::InvalidPledge);
    }
    if versions
        .windows(2)
        .any(|pair| pair[0].effective_from >= pair[1].effective_from)
    {
        return Err(AccountingError::InvalidPledge);
    }
    let mut result = Vec::new();
    let mut start = pledge.start_date;
    let mut version_index = 0;
    let mut events = pledge.events.clone();
    events.sort_by_key(|event| event.effective_date);
    if events
        .windows(2)
        .any(|pair| pair[0].effective_date >= pair[1].effective_date)
    {
        return Err(AccountingError::InvalidPledge);
    }
    let mut event_index = 0;
    let mut paused = false;
    let mut cancelled = false;
    while start <= horizon && pledge.end_date.is_none_or(|end| start <= end) && !cancelled {
        if let Some(event) = events.get(event_index)
            && event.effective_date == start
        {
            match (paused, event.action) {
                (false, StateAction::Pause) => {
                    paused = true;
                    event_index += 1;
                    continue;
                }
                (true, StateAction::Resume) => {
                    paused = false;
                    event_index += 1;
                    continue;
                }
                _ => {}
            }
        }

        if paused {
            let Some(event) = events.get(event_index) else {
                break;
            };
            if event.action != StateAction::Resume || event.effective_date <= start {
                return Err(AccountingError::InvalidPledge);
            }
            start = event.effective_date;
            continue;
        }
        if *remaining_budget == 0 {
            return Err(AccountingError::TooManyPeriods);
        }
        *remaining_budget -= 1;
        let end = period_end(start, pledge.frequency)?;
        let end = pledge
            .end_date
            .map_or(end, |pledge_end| end.min(pledge_end));
        let mut cancelled_here = false;
        if let Some(event) = events.get(event_index)
            && event.effective_date <= end
        {
            if event.action == StateAction::Cancel && event.effective_date == end {
                cancelled_here = true;
            } else {
                return Err(AccountingError::InvalidPledge);
            }
        }
        if versions
            .get(version_index + 1)
            .is_some_and(|next| next.effective_from <= end)
        {
            if versions[version_index + 1].effective_from != start {
                return Err(AccountingError::InvalidPledge);
            }
            version_index += 1;
        }
        result.push(Period {
            pledge_id: pledge.id,
            project_id: pledge.project_id,
            start,
            end,
            amount: versions[version_index].amount,
            paid: 0,
        });
        if cancelled_here {
            cancelled = true;
            event_index += 1;
        }
        start = end.succ_opt().ok_or(AccountingError::Overflow)?;
    }
    if events
        .get(event_index)
        .is_some_and(|event| event.effective_date <= horizon)
    {
        return Err(AccountingError::InvalidPledge);
    }
    if versions
        .last()
        .is_some_and(|version| version.effective_from <= horizon && version.effective_from >= start)
    {
        return Err(AccountingError::InvalidPledge);
    }
    Ok(result)
}

/// Replays recorded payments through `as_of`, allocating oldest eligible periods first.
/// The future horizon is bounded by the total payment and the smallest pledge amount.
pub fn replay(
    pledges: &[Pledge],
    payments: &[Payment],
    as_of: NaiveDate,
) -> Result<Replay, AccountingError> {
    let mut total_payment = 0;
    for payment in payments
        .iter()
        .filter(|payment| !payment.voided && payment.date <= as_of)
    {
        if payment.amount <= 0 || payment.amount > MAX_MONEY {
            return Err(AccountingError::InvalidAmount);
        }
        add(&mut total_payment, payment.amount)?;
    }
    let min_amount = pledges
        .iter()
        .flat_map(|pledge| pledge.versions.iter().map(|v| v.amount))
        .min()
        .unwrap_or(1);
    if min_amount <= 0 {
        return Err(AccountingError::InvalidPledge);
    }
    let future_count = total_payment / min_amount + 1;
    let future_days = future_count.saturating_mul(31).min(3_100_000);
    let horizon = as_of
        .checked_add_signed(Duration::days(future_days))
        .unwrap_or(NaiveDate::MAX);
    let mut budget = MAX_PERIODS;
    let mut periods = Vec::new();
    for pledge in pledges {
        periods.extend(periods_for(pledge, horizon, &mut budget)?);
    }
    periods.sort_by_key(|p| (p.end, p.start, p.pledge_id));
    let mut ordered = payments
        .iter()
        .filter(|payment| !payment.voided && payment.date <= as_of)
        .collect::<Vec<_>>();
    ordered.sort_by_key(|payment| (payment.date, payment.id));
    let mut allocations = Vec::new();
    let mut payment_credit = BTreeMap::new();
    for payment in ordered {
        let mut remaining = payment.amount;
        for period in periods
            .iter_mut()
            .filter(|p| payment.project_id.is_none_or(|id| p.project_id == id))
        {
            if remaining == 0 {
                break;
            }
            let unpaid = period.amount - period.paid;
            if unpaid == 0 {
                continue;
            }
            let amount = remaining.min(unpaid);
            period.paid += amount;
            remaining -= amount;
            allocations.push(Allocation {
                payment_id: payment.id,
                pledge_id: period.pledge_id,
                period_start: period.start,
                period_end: period.end,
                amount,
            });
        }
        payment_credit.insert(payment.id, remaining);
    }
    let mut pledge_balances: BTreeMap<Uuid, Balance> = pledges
        .iter()
        .map(|pledge| (pledge.id, Balance::default()))
        .collect();
    let mut balance = Balance::default();
    for period in &periods {
        let item = pledge_balances
            .get_mut(&period.pledge_id)
            .ok_or(AccountingError::InvalidPledge)?;
        if period.start <= as_of {
            add(&mut item.demand, period.amount)?;
            add(&mut item.paid, period.paid)?;
        }
        if period.end < as_of {
            add(&mut item.debt, period.amount - period.paid)?;
        } else if period.end == as_of {
            add(&mut item.due_today, period.amount - period.paid)?;
            add(&mut item.advance, period.paid)?;
        } else {
            add(&mut item.advance, period.paid)?;
        }
    }
    for credit in payment_credit.values() {
        add(&mut balance.unallocated_credit, *credit)?;
    }
    for item in pledge_balances.values() {
        add(&mut balance.demand, item.demand)?;
        add(&mut balance.paid, item.paid)?;
        add(&mut balance.debt, item.debt)?;
        add(&mut balance.due_today, item.due_today)?;
        add(&mut balance.advance, item.advance)?;
    }
    Ok(Replay {
        allocations,
        payment_credit,
        pledge_balances,
        balance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }
    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, day).unwrap()
    }
    fn pledge(end_date: Option<NaiveDate>) -> Pledge {
        Pledge {
            id: id(1),
            project_id: id(2),
            frequency: Frequency::Daily,
            start_date: date(1),
            end_date,
            versions: vec![AmountVersion {
                id: id(3),
                amount: 1_000,
                effective_from: date(1),
            }],
            events: vec![],
        }
    }
    fn payment() -> Payment {
        Payment {
            id: id(4),
            project_id: None,
            amount: 15_000,
            date: date(11),
            voided: false,
        }
    }

    #[test]
    fn acceptance_vector_and_void() {
        let before = replay(&[pledge(None)], &[], date(11)).unwrap();
        assert_eq!(
            (before.balance.debt, before.balance.due_today),
            (10_000, 1_000)
        );
        let after = replay(&[pledge(None)], &[payment()], date(11)).unwrap();
        assert_eq!(
            (
                after.balance.debt,
                after.balance.due_today,
                after.balance.advance,
                after.balance.unallocated_credit
            ),
            (0, 0, 5_000, 0)
        );
        assert_eq!(
            after
                .allocations
                .iter()
                .map(|item| item.amount)
                .sum::<i64>(),
            15_000
        );
        let mut voided = payment();
        voided.voided = true;
        assert_eq!(
            replay(&[pledge(None)], &[voided], date(11))
                .unwrap()
                .balance,
            before.balance
        );
    }

    #[test]
    fn restricted_payment_and_credit() {
        let mut restricted = payment();
        restricted.project_id = Some(id(9));
        let result = replay(&[pledge(None)], &[restricted], date(11)).unwrap();
        assert_eq!(result.balance.unallocated_credit, 15_000);
        assert_eq!(result.balance.debt, 10_000);
        let result = replay(&[pledge(Some(date(11)))], &[payment()], date(11)).unwrap();
        assert_eq!(result.balance.unallocated_credit, 4_000);
    }

    #[test]
    fn weekly_partial_period_is_full_charge() {
        let mut p = pledge(Some(date(4)));
        p.frequency = Frequency::Weekly;
        p.start_date = date(3);
        p.versions[0].effective_from = date(3);
        let result = replay(&[p], &[], date(4)).unwrap();
        assert_eq!(result.balance.demand, 1_000);
        assert_eq!(result.balance.due_today, 1_000);
    }

    #[test]
    fn pause_resume_and_amount_version_preserve_history() {
        let mut p = pledge(Some(date(15)));
        p.events = vec![
            StateEvent {
                action: StateAction::Pause,
                effective_date: date(5),
            },
            StateEvent {
                action: StateAction::Resume,
                effective_date: date(10),
            },
        ];
        p.versions.push(AmountVersion {
            id: id(5),
            amount: 2_000,
            effective_from: date(10),
        });
        let result = replay(&[p], &[], date(15)).unwrap();
        assert_eq!(result.balance.demand, 16_000);
        assert_eq!(result.balance.debt, 14_000);
        assert_eq!(result.balance.due_today, 2_000);
    }

    #[test]
    fn rejects_mid_period_pause_and_overflow() {
        let mut p = pledge(None);
        p.frequency = Frequency::Weekly;
        p.events = vec![StateEvent {
            action: StateAction::Pause,
            effective_date: date(3),
        }];
        assert_eq!(
            replay(&[p], &[], date(4)).unwrap_err(),
            AccountingError::InvalidPledge
        );
        let payments = [
            Payment {
                id: id(6),
                project_id: None,
                amount: MAX_MONEY,
                date: date(1),
                voided: false,
            },
            Payment {
                id: id(7),
                project_id: None,
                amount: 1,
                date: date(1),
                voided: false,
            },
        ];
        assert_eq!(
            replay(&[], &payments, date(1)).unwrap_err(),
            AccountingError::Overflow
        );
    }
}

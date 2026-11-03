//! Invoice lifecycle with line items, payments and overdue sweeps.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Time is an explicit tick supplied by the caller. Invoices are
//! built from priced line items; payments allocate against the total and
//! close the invoice only when it is fully paid.

use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_INVOICES: usize = 10_000;
pub const MAX_LINE_ITEMS: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum InvoiceError {
    #[error("billing capacity must be between 1 and {MAX_INVOICES}")]
    InvalidCapacity,
    #[error("billing is full (max {MAX_INVOICES} invoices)")]
    BillingFull,
    #[error("invoice number must not be empty")]
    InvalidInvoiceNumber,
    #[error("an invoice needs between 1 and {MAX_LINE_ITEMS} line items")]
    InvalidLineItems,
    #[error("line quantities and unit prices must be greater than zero")]
    ZeroAmount,
    #[error("line total overflows")]
    Overflow,
    #[error("unknown invoice {0}")]
    UnknownInvoice(u64),
    #[error("invoice {0} is not open")]
    InvoiceNotOpen(u64),
    #[error("payment for invoice {0} must be greater than zero")]
    ZeroPayment(u64),
    #[error("payment {2} exceeds the remaining {3} on invoice {0}")]
    Overpayment {
        invoice: u64,
        payment: u64,
        remaining: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineItem {
    pub label: String,
    pub quantity: u32,
    pub unit_price: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvoiceState {
    Open,
    Paid,
    Void,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invoice {
    pub id: u64,
    pub number: String,
    pub items: Vec<LineItem>,
    pub created_tick: u64,
    pub due_tick: u64,
    pub state: InvoiceState,
    pub paid_amount: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaymentOutcome {
    PartiallyPaid { remaining: u64 },
    FullyPaid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BillingStats {
    pub open: usize,
    pub overdue: usize,
    pub paid: usize,
    pub void: usize,
}

#[derive(Debug)]
pub struct Billing {
    max_invoices: usize,
    next_id: u64,
    invoices: BTreeMap<u64, Invoice>,
}

impl Billing {
    pub fn new(max_invoices: usize) -> Result<Self, InvoiceError> {
        if max_invoices == 0 || max_invoices > MAX_INVOICES {
            return Err(InvoiceError::InvalidCapacity);
        }
        Ok(Self {
            max_invoices,
            next_id: 1,
            invoices: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.invoices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.invoices.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Invoice> {
        self.invoices.get(&id)
    }

    /// Creates an open invoice from priced line items.
    pub fn create(
        &mut self,
        number: &str,
        items: Vec<LineItem>,
        created_tick: u64,
        due_tick: u64,
    ) -> Result<u64, InvoiceError> {
        if number.is_empty() {
            return Err(InvoiceError::InvalidInvoiceNumber);
        }
        if items.is_empty() || items.len() > MAX_LINE_ITEMS {
            return Err(InvoiceError::InvalidLineItems);
        }
        for item in &items {
            if item.quantity == 0 || item.unit_price == 0 {
                return Err(InvoiceError::ZeroAmount);
            }
        }
        if self.invoices.len() >= self.max_invoices {
            return Err(InvoiceError::BillingFull);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.invoices.insert(
            id,
            Invoice {
                id,
                number: number.to_string(),
                items,
                created_tick,
                due_tick,
                state: InvoiceState::Open,
                paid_amount: 0,
            },
        );
        Ok(id)
    }

    /// Total invoiced amount in minor units.
    pub fn total(&self, id: u64) -> Result<u64, InvoiceError> {
        let invoice = self
            .invoices
            .get(&id)
            .ok_or(InvoiceError::UnknownInvoice(id))?;
        let mut total = 0u64;
        for item in &invoice.items {
            let line = u64::from(item.quantity)
                .checked_mul(item.unit_price)
                .ok_or(InvoiceError::Overflow)?;
            total = total.checked_add(line).ok_or(InvoiceError::Overflow)?;
        }
        Ok(total)
    }

    /// Allocates a payment against an open invoice. The invoice closes as
    /// Paid only when the total is reached; overpayment is rejected.
    pub fn record_payment(
        &mut self,
        id: u64,
        amount: u64,
    ) -> Result<PaymentOutcome, InvoiceError> {
        if amount == 0 {
            return Err(InvoiceError::ZeroPayment(id));
        }
        let invoice = self
            .invoices
            .get_mut(&id)
            .ok_or(InvoiceError::UnknownInvoice(id))?;
        if invoice.state != InvoiceState::Open {
            return Err(InvoiceError::InvoiceNotOpen(id));
        }
        let total = self.total(id)?;
        let remaining = total - invoice.paid_amount;
        if amount > remaining {
            return Err(InvoiceError::Overpayment {
                invoice: id,
                payment: amount,
                remaining,
            });
        }
        invoice.paid_amount += amount;
        if invoice.paid_amount == total {
            invoice.state = InvoiceState::Paid;
            Ok(PaymentOutcome::FullyPaid)
        } else {
            Ok(PaymentOutcome::PartiallyPaid {
                remaining: total - invoice.paid_amount,
            })
        }
    }

    /// Voids an open invoice; a paid invoice can no longer be voided.
    pub fn void(&mut self, id: u64) -> Result<(), InvoiceError> {
        let invoice = self
            .invoices
            .get_mut(&id)
            .ok_or(InvoiceError::UnknownInvoice(id))?;
        if invoice.state != InvoiceState::Open {
            return Err(InvoiceError::InvoiceNotOpen(id));
        }
        invoice.state = InvoiceState::Void;
        Ok(())
    }

    /// Ids of open invoices whose due tick has passed.
    pub fn overdue(&self, now: u64) -> Vec<u64> {
        self.invoices
            .values()
            .filter(|invoice| {
                invoice.state == InvoiceState::Open && invoice.due_tick <= now
            })
            .map(|invoice| invoice.id)
            .collect()
    }

    pub fn stats(&self, now: u64) -> BillingStats {
        let mut stats = BillingStats::default();
        for invoice in self.invoices.values() {
            match invoice.state {
                InvoiceState::Open => {
                    stats.open += 1;
                    if invoice.due_tick <= now {
                        stats.overdue += 1;
                    }
                }
                InvoiceState::Paid => stats.paid += 1,
                InvoiceState::Void => stats.void += 1,
            }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, quantity: u32, unit_price: u64) -> LineItem {
        LineItem {
            label: label.to_string(),
            quantity,
            unit_price,
        }
    }

    fn billing() -> Billing {
        Billing::new(4).unwrap()
    }

    #[test]
    fn validates_capacity_and_invoice() {
        assert_eq!(
            Billing::new(0).unwrap_err(),
            InvoiceError::InvalidCapacity
        );
        assert_eq!(
            Billing::new(MAX_INVOICES + 1).unwrap_err(),
            InvoiceError::InvalidCapacity
        );
        let mut billing = billing();
        assert_eq!(
            billing.create("", vec![item("a", 1, 10)], 0, 5).unwrap_err(),
            InvoiceError::InvalidInvoiceNumber
        );
        assert_eq!(
            billing.create("INV-1", Vec::new(), 0, 5).unwrap_err(),
            InvoiceError::InvalidLineItems
        );
        assert_eq!(
            billing
                .create("INV-1", vec![item("a", 0, 10)], 0, 5)
                .unwrap_err(),
            InvoiceError::ZeroAmount
        );
    }

    #[test]
    fn billing_full_is_rejected() {
        let mut billing = Billing::new(1).unwrap();
        billing
            .create("INV-1", vec![item("a", 1, 10)], 0, 5)
            .unwrap();
        assert_eq!(
            billing
                .create("INV-2", vec![item("a", 1, 10)], 0, 5)
                .unwrap_err(),
            InvoiceError::BillingFull
        );
        assert_eq!(billing.len(), 1);
        assert!(!billing.is_empty());
    }

    #[test]
    fn totals_sum_line_items() {
        let mut billing = billing();
        let id = billing
            .create(
                "INV-1",
                vec![item("storage", 3, 100), item("bandwidth", 2, 25)],
                0,
                50,
            )
            .unwrap();
        assert_eq!(billing.total(id).unwrap(), 350);
    }

    #[test]
    fn partial_then_full_payment_closes_invoice() {
        let mut billing = billing();
        let id = billing
            .create("INV-1", vec![item("storage", 2, 100)], 0, 50)
            .unwrap();
        assert_eq!(
            billing.record_payment(id, 0).unwrap_err(),
            InvoiceError::ZeroPayment(id)
        );
        assert_eq!(
            billing.record_payment(id, 50).unwrap(),
            PaymentOutcome::PartiallyPaid { remaining: 150 }
        );
        assert_eq!(
            billing.record_payment(id, 200).unwrap_err(),
            InvoiceError::Overpayment {
                invoice: id,
                payment: 200,
                remaining: 150
            }
        );
        assert_eq!(
            billing.record_payment(id, 150).unwrap(),
            PaymentOutcome::FullyPaid
        );
        assert_eq!(billing.get(id).unwrap().state, InvoiceState::Paid);
        assert_eq!(
            billing.record_payment(id, 1).unwrap_err(),
            InvoiceError::InvoiceNotOpen(id)
        );
    }

    #[test]
    fn void_and_unknown_invoice() {
        let mut billing = billing();
        let id = billing
            .create("INV-1", vec![item("storage", 1, 100)], 0, 50)
            .unwrap();
        billing.void(id).unwrap();
        assert_eq!(billing.get(id).unwrap().state, InvoiceState::Void);
        assert_eq!(
            billing.void(id).unwrap_err(),
            InvoiceError::InvoiceNotOpen(id)
        );
        assert_eq!(
            billing.total(99).unwrap_err(),
            InvoiceError::UnknownInvoice(99)
        );
    }

    #[test]
    fn overdue_sweep_and_stats() {
        let mut billing = billing();
        let a = billing
            .create("INV-1", vec![item("storage", 1, 100)], 0, 10)
            .unwrap();
        let b = billing
            .create("INV-2", vec![item("storage", 1, 100)], 0, 100)
            .unwrap();
        let c = billing
            .create("INV-3", vec![item("storage", 1, 100)], 0, 100)
            .unwrap();
        billing.record_payment(c, 100).unwrap();
        assert_eq!(billing.overdue(10), vec![a]);
        let stats = billing.stats(10);
        assert_eq!(stats.open, 2);
        assert_eq!(stats.overdue, 1);
        assert_eq!(stats.paid, 1);
        assert_eq!(stats.void, 0);
        assert_eq!(billing.get(b).unwrap().state, InvoiceState::Open);
    }
}

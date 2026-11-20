use fix_dict::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Acknowledge,
    PartialFill,
    FullFill,
    Reject,
    PendingNew,
    PendingCancel,
    Cancel,
    PendingReplace,
    Replace,
    RejectCancel,
    RejectReplace,
    Status,
    Expire,
    DoneForDay,
    Stopped,
    Suspended,
    Calculated,
    Restate,
    Correct,
    Bust,
}

use Action::{
    Acknowledge, Bust, Calculated, Cancel, Correct, DoneForDay, Expire, FullFill, PartialFill,
    PendingCancel, PendingNew, PendingReplace, Reject, RejectCancel, RejectReplace, Replace,
    Restate, Status, Stopped, Suspended,
};

pub(super) const ORDER: &[Action] = &[
    Acknowledge,
    PartialFill,
    FullFill,
    Reject,
    PendingNew,
    Cancel,
    Status,
    Expire,
    DoneForDay,
    Stopped,
    Suspended,
    Calculated,
    Restate,
    Correct,
    Bust,
];
pub(super) const CANCEL: &[Action] = &[Cancel, RejectCancel, PendingCancel];
pub(super) const REPLACE: &[Action] = &[Replace, RejectReplace, PendingReplace];
pub(super) const TRADE: &[Action] = &[Correct, Bust];

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Acknowledge => "Acknowledge",
            PartialFill => "Partial fill",
            FullFill => "Full fill",
            Reject => "Reject order",
            PendingNew => "Pending new",
            PendingCancel => "Pending cancel",
            Cancel => "Cancel order",
            PendingReplace => "Pending replace",
            Replace => "Replace order",
            RejectCancel => "Reject cancel request",
            RejectReplace => "Reject replace request",
            Status => "Order status",
            Expire => "Expire",
            DoneForDay => "Done for day",
            Stopped => "Stopped",
            Suspended => "Suspended",
            Calculated => "Calculated",
            Restate => "Restate",
            Correct => "Trade correction",
            Bust => "Trade bust",
        }
    }

    pub fn less_common(self) -> bool {
        matches!(
            self,
            Expire | DoneForDay | Stopped | Suspended | Calculated | Restate | Correct | Bust
        )
    }

    pub(super) fn msg_type(self) -> &'static str {
        if self.cancel_reject().is_some() {
            "9"
        } else {
            "8"
        }
    }

    pub(super) fn cancel_reject(self) -> Option<&'static str> {
        match self {
            RejectCancel => Some("1"),
            RejectReplace => Some("2"),
            _ => None,
        }
    }

    pub(super) fn status(self, version: Version) -> &'static str {
        match self {
            Acknowledge => "0",
            PartialFill => "1",
            FullFill => "2",
            DoneForDay => "3",
            Cancel => "4",
            Replace if version != Version::Fix44 => "5",
            PendingCancel => "6",
            Stopped => "7",
            Reject => "8",
            Suspended => "9",
            PendingNew => "A",
            Calculated => "B",
            Expire => "C",
            PendingReplace => "E",
            _ => "",
        }
    }

    pub(super) fn exec_type(self, version: Version, status: &str, reference_type: &str) -> String {
        match self {
            Restate => "D",
            Replace => "5",
            Correct | Bust if version == Version::Fix42 => reference_type,
            Status if version == Version::Fix42 => status,
            PartialFill | FullFill if version != Version::Fix42 => "F",
            Correct => "G",
            Bust => "H",
            Status => "I",
            _ => self.status(version),
        }
        .into()
    }

    pub(super) fn transaction(self) -> &'static str {
        match self {
            Bust => "1",
            Correct => "2",
            Status => "3",
            _ => "0",
        }
    }

    pub(super) fn references_trade(self) -> bool {
        TRADE.contains(&self)
    }

    pub(super) fn has_trade_values(self) -> bool {
        matches!(self, PartialFill | FullFill | Correct)
    }
}

//! Tag numbers and message types that the session layer uses.

pub const BEGIN_SEQ_NO: u32 = 7;
pub const BEGIN_STRING: u32 = 8;
pub const BODY_LENGTH: u32 = 9;
pub const CHECK_SUM: u32 = 10;
pub const END_SEQ_NO: u32 = 16;
pub const MSG_SEQ_NUM: u32 = 34;
pub const MSG_TYPE: u32 = 35;
pub const NEW_SEQ_NO: u32 = 36;
pub const POSS_DUP_FLAG: u32 = 43;
pub const REF_SEQ_NUM: u32 = 45;
pub const SENDER_COMP_ID: u32 = 49;
pub const SENDING_TIME: u32 = 52;
pub const TARGET_COMP_ID: u32 = 56;
pub const TEXT: u32 = 58;
pub const POSS_RESEND: u32 = 97;
pub const ENCRYPT_METHOD: u32 = 98;
pub const HEART_BT_INT: u32 = 108;
pub const TEST_REQ_ID: u32 = 112;
pub const ORIG_SENDING_TIME: u32 = 122;
pub const GAP_FILL_FLAG: u32 = 123;
pub const RESET_SEQ_NUM_FLAG: u32 = 141;
pub const REF_TAG_ID: u32 = 371;
pub const REF_MSG_TYPE: u32 = 372;
pub const SESSION_REJECT_REASON: u32 = 373;
pub const USERNAME: u32 = 553;
pub const PASSWORD: u32 = 554;

/// Length fields and the data fields that follow them. A data field can
/// contain SOH, so the decoder reads it by length.
pub const DATA_FIELDS: [(u32, u32); 16] = [
    (90, 91),
    (93, 89),
    (95, 96),
    (212, 213),
    (348, 349),
    (350, 351),
    (352, 353),
    (354, 355),
    (356, 357),
    (358, 359),
    (360, 361),
    (362, 363),
    (364, 365),
    (445, 446),
    (618, 619),
    (621, 622),
];

/// Returns the data tag that `length_tag` gives the length of.
pub fn data_tag_for(length_tag: u32) -> Option<u32> {
    DATA_FIELDS
        .iter()
        .find(|(length, _)| *length == length_tag)
        .map(|(_, data)| *data)
}

/// Session message types.
pub mod msg_type {
    pub const HEARTBEAT: &[u8] = b"0";
    pub const TEST_REQUEST: &[u8] = b"1";
    pub const RESEND_REQUEST: &[u8] = b"2";
    pub const REJECT: &[u8] = b"3";
    pub const SEQUENCE_RESET: &[u8] = b"4";
    pub const LOGOUT: &[u8] = b"5";
    pub const LOGON: &[u8] = b"A";

    /// Returns true for the session (administrative) message types.
    pub fn is_admin(msg_type: &[u8]) -> bool {
        matches!(msg_type, b"0" | b"1" | b"2" | b"3" | b"4" | b"5" | b"A")
    }
}

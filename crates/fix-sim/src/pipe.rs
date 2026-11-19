//! One direction of a simulated TCP connection: bytes arrive in order, after
//! a random delay, in chunks of random size.

use std::collections::VecDeque;

use fix_codec::UtcMillis;

use crate::rng::Rng;

const LATENCY_MS_MAX: u64 = 300;

#[derive(Debug)]
pub enum Item {
    Bytes(Vec<u8>),
    /// The sender closed the connection.
    Eof,
}

#[derive(Debug)]
struct Entry {
    at: UtcMillis,
    item: Item,
}

#[derive(Debug, Default)]
pub struct Pipe {
    queue: VecDeque<Entry>,
    /// The arrival time of the last item. Arrival times never go back, so the
    /// bytes stay in order.
    last_at: UtcMillis,
    stall_until: UtcMillis,
    /// Nobody reads this pipe any more. It drops all writes.
    dead: bool,
}

impl Pipe {
    /// Queues `bytes`, cut into chunks of random size.
    pub fn write(&mut self, bytes: &[u8], now: UtcMillis, rng: &mut Rng) {
        let mut rest = bytes;
        while !rest.is_empty() {
            let size = match rng.below(4) {
                0 => rng.range(1, 3),
                1 => rng.range(4, 20),
                2 => rng.range(21, 80),
                _ => u64::MAX,
            };
            let size = usize::try_from(size).unwrap_or(usize::MAX).min(rest.len());
            let (chunk, tail) = rest.split_at(size);
            self.push(Item::Bytes(chunk.to_vec()), now, rng);
            rest = tail;
        }
    }

    /// Queues the end of the stream after the bytes that are already queued.
    pub fn close(&mut self, now: UtcMillis, rng: &mut Rng) {
        self.push(Item::Eof, now, rng);
    }

    /// Drops everything in flight and all later writes.
    pub fn kill(&mut self) {
        self.dead = true;
        self.queue.clear();
    }

    /// Holds back all bytes, those in flight and those written until then, so
    /// that none arrives before `until`.
    pub fn stall(&mut self, until: UtcMillis) {
        self.stall_until = self.stall_until.max(until);
        for entry in &mut self.queue {
            entry.at = entry.at.max(self.stall_until);
        }
        self.last_at = self.last_at.max(self.stall_until);
    }

    pub fn next_at(&self) -> Option<UtcMillis> {
        self.queue.front().map(|entry| entry.at)
    }

    /// Takes the first item that has arrived at `now`.
    pub fn pop_due(&mut self, now: UtcMillis) -> Option<Item> {
        if self.next_at()? > now {
            return None;
        }
        self.queue.pop_front().map(|entry| entry.item)
    }

    fn push(&mut self, item: Item, now: UtcMillis, rng: &mut Rng) {
        if self.dead {
            return;
        }
        let arrival = UtcMillis(now.0 + rng.range(1, LATENCY_MS_MAX));
        let at = arrival.max(self.last_at).max(self.stall_until);
        self.last_at = at;
        self.queue.push_back(Entry { at, item });
    }
}

#[cfg(test)]
mod tests {
    use super::{Item, Pipe};
    use crate::rng::Rng;
    use fix_codec::UtcMillis;

    fn collect(pipe: &mut Pipe, now: UtcMillis) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(Item::Bytes(bytes)) = pipe.pop_due(now) {
            out.extend(bytes);
        }
        out
    }

    #[test]
    fn bytes_arrive_in_order_and_complete() {
        let mut rng = Rng::new(3);
        let mut pipe = Pipe::default();
        let sent: Vec<u8> = (0..=255).collect();
        pipe.write(&sent, UtcMillis(1_000), &mut rng);
        pipe.write(&sent, UtcMillis(1_001), &mut rng);
        let got = collect(&mut pipe, UtcMillis(1_000_000));
        assert_eq!(got.len(), 512);
        assert_eq!(&got[..256], sent.as_slice());
        assert_eq!(&got[256..], sent.as_slice());
    }

    #[test]
    fn stall_delays_old_and_new_bytes() {
        let mut rng = Rng::new(4);
        let mut pipe = Pipe::default();
        pipe.write(b"early", UtcMillis(0), &mut rng);
        pipe.stall(UtcMillis(50_000));
        pipe.write(b"late", UtcMillis(10), &mut rng);
        assert_eq!(collect(&mut pipe, UtcMillis(49_999)), Vec::<u8>::new());
        assert_eq!(collect(&mut pipe, UtcMillis(50_000)), b"earlylate");
    }

    #[test]
    fn a_killed_pipe_drops_writes() {
        let mut rng = Rng::new(5);
        let mut pipe = Pipe::default();
        pipe.write(b"gone", UtcMillis(0), &mut rng);
        pipe.kill();
        pipe.write(b"never", UtcMillis(0), &mut rng);
        assert!(pipe.next_at().is_none());
    }
}

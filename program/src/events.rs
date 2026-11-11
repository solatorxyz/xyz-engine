//! Fill and liquidation events, written to the transaction log as one `sol_log_data` record per instruction. Buffers are
//! fixed-size and on the stack (the program has no allocator): a taker that crosses more makers than a buffer holds
//! still fills, and the record says how many entries were dropped.

use super::*;

#[inline(always)]
pub(crate) fn emit(parts: &[&[u8]]) {
    #[cfg(target_os = "solana")]
    unsafe {
        pinocchio::syscalls::sol_log_data(parts.as_ptr() as *const u8, parts.len() as u64)
    };
    #[cfg(not(target_os = "solana"))]
    let _ = parts;
}

pub(crate) const EV_MAX: usize = 16;

pub(crate) const EV_ENTRY: usize = 56; // maker 32 | px 8 | qty 8 | order id 8

/// Batched fill event: one sol_log_data per take (or per 16 fills):
/// [EVENTS_FILL, taker_side, taker 32, n u8, n x (maker 32, px u64, qty u64, id u64)]
pub(crate) struct EvBuf {
    b: [u8; 35 + EV_MAX * EV_ENTRY],
    n: usize,
}

impl EvBuf {
    pub(crate) fn new(taker: &Key, taker_bid: bool) -> Self {
        let mut b = [0u8; 35 + EV_MAX * EV_ENTRY];
        b[0] = EVENTS_FILL as u8;
        b[1] = if taker_bid { 0 } else { 1 };
        b[2..34].copy_from_slice(taker);
        EvBuf { b, n: 0 }
    }
    #[inline(always)]
    pub(crate) fn push(&mut self, maker: &Key, px: u64, q: u64, id: u64) {
        let o = 35 + self.n * EV_ENTRY;
        self.b[o..o + 32].copy_from_slice(maker);
        self.b[o + 32..o + 40].copy_from_slice(&px.to_le_bytes());
        self.b[o + 40..o + 48].copy_from_slice(&q.to_le_bytes());
        self.b[o + 48..o + 56].copy_from_slice(&id.to_le_bytes());
        self.n += 1;
        if self.n == EV_MAX {
            self.flush();
        }
    }
    pub(crate) fn flush(&mut self) {
        if self.n > 0 {
            self.b[34] = self.n as u8;
            emit(&[&self.b[..35 + self.n * EV_ENTRY]]);
            self.n = 0;
        }
    }
}

/// Fast-path fill event, one sol_log_data per take (or per 32 fills), 16 bytes a fill:
/// [EVENTS_FAST_FILL, taker_side, taker 32, n u8, n x (px u32, qty u32, id u64)], id = maker index << 32 | quote generation.
/// The maker is FastBook.makers[index] of the book in the same transaction (indices never move: makers are only appended).
/// A take_fast_batch logs one EVENTS_FAST_BATCH for all its takers: [tag, 0, fast book 32, n u8, entries], with
/// id = order slot << 40 | taker side (1 = sell) << 39 | maker index << 32 | gen (order slot k = the k-th order of the batch).
/// The buffer is not zeroed: only the bytes written are logged.
pub(crate) const FEV_MAX: usize = 32;

pub(crate) const FEV_ENTRY: usize = 16;

pub(crate) struct FastEv {
    b: core::mem::MaybeUninit<[u8; 35 + FEV_MAX * FEV_ENTRY]>,
    n: usize,
}

impl FastEv {
    #[inline(always)]
    pub(crate) fn new(taker: &Key, taker_bid: bool) -> Self {
        FastEv::with(spec::EVENTS_FAST_FILL as u8, if taker_bid { 0 } else { 1 }, taker)
    }
    #[inline(always)]
    pub(crate) fn with(tag: u8, b1: u8, k: &Key) -> Self {
        let mut e = FastEv { b: core::mem::MaybeUninit::uninit(), n: 0 };
        let p = e.b.as_mut_ptr() as *mut u8;
        unsafe {
            *p = tag;
            *p.add(1) = b1;
            core::ptr::copy_nonoverlapping(k.as_ptr(), p.add(2), 32);
        }
        e
    }
    #[inline(always)]
    pub(crate) fn push(&mut self, px: u64, q: u64, id: u64) {
        let p = unsafe { (self.b.as_mut_ptr() as *mut u8).add(35 + self.n * FEV_ENTRY) };
        unsafe {
            (p as *mut u32).write_unaligned(px as u32);
            (p.add(4) as *mut u32).write_unaligned(q as u32);
            (p.add(8) as *mut u64).write_unaligned(id);
        }
        self.n += 1;
        if self.n == FEV_MAX {
            self.flush();
        }
    }
    pub(crate) fn flush(&mut self) {
        if self.n == 0 {
            return;
        }
        let p = self.b.as_mut_ptr() as *mut u8;
        unsafe {
            *p.add(34) = self.n as u8;
            emit(&[core::slice::from_raw_parts(p, 35 + self.n * FEV_ENTRY)]);
        }
        self.n = 0;
    }
}

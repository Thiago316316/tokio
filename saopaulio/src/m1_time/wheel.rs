//! A hierarchical timing wheel: six levels of 64 slots.
//!
//! NOTE: the prose below still describes the single-level version of step 2.
//! The levels are implemented and tested but not yet written up here.
//!
//! # Why not a heap
//!
//! A `BinaryHeap` of `(deadline, id)` peeks the minimum in O(1), but pays
//! O(log n) comparisons per insert and per pop, and cannot cancel at all: the
//! heap property orders parent-to-child only, so no arithmetic locates an
//! arbitrary entry. Removal is O(n) by construction.
//!
//! The wheel replaces searching with addressing.
//!
//! # The stripe
//!
//! 64 slots, one millisecond each. A deadline's slot is `deadline % 64` — the
//! same digit extraction as reading the hundreds place of 5789 with
//! `(5789 / 100) % 10`, but in base 64, where `/` and `%` are `>> 6` and `& 63`.
//!
//! `% 64` keeps a position on the ring and throws the lap count away. That is
//! only unambiguous while at most one lap is in flight, which is exactly what
//! `TooFar` / `Elapsed` enforce: the live window is `[elapsed, elapsed + 64)`,
//! one lap wide, so each slot has at most one possible deadline.
//!
//! `insert` and `next_deadline` are therefore O(1) — arithmetic, not search.
//!
//! # `occupied`
//!
//! Two parallel structures of length 64: `slots` holds the timers, `occupied`
//! is a 64-bit summary where bit n is set **iff** `slots[n]` is non-empty.
//! `1u64 << n` is an address, not a quantity — the mask's zeros are what leave
//! the other bits alone.
//!
//! The bitmask exists for one reader, `next_deadline`, which a runtime calls
//! before every park to decide how long it may sleep. `rotate_right` +
//! `trailing_zeros` searches all 64 slots in ~3 instructions; `rotate` rather
//! than `shift` so slots behind "now" wrap to the far end as next revolution
//! instead of being deleted.
//!
//! Every mutation of a slot must restore the invariant. `cancel` is the only
//! site where the clear is conditional — removing one of several entries must
//! leave the bit set. Drift there is silent and fatal (a timer that never
//! fires), which is what `next_deadline_never_overshoots_the_oracle` guards.
//!
//! # Still missing
//!
//! - Within a slot, removal is O(len) plus a `Vec` shift. Tokio's entries carry
//!   their own `prev`/`next`, so an entry unlinks itself in O(1).

use std::array;

 struct Entry<T>{
    id: u64,
    deadline: u64,
    payload: T,
 }

/* pub struct Wheel<T>{ old wheel
    slots: [Vec<Entry<T>>; 64],
    elapsed: u64, // = next tick not yet processed
    next_id: u64,
    occupied: u64, // bitmask of occupied slots
 }*/

#[derive(Debug, PartialEq, Eq)]
pub enum InsertError{
    TooFar,
    Elapsed,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TimerHandle{
    id: u64,
    deadline: u64,
}



const LEVELS: usize = 6;
const SLOTS: usize = 64;
const SLOT_MASK: u64 = 63;
const MAX_DURATION: u64 = (1 << 36) - 1;

fn level_for(elapsed: u64, deadline: u64) -> usize{
    let mut masked = (elapsed ^ deadline) | SLOT_MASK;
    if masked >= MAX_DURATION {
        masked = MAX_DURATION - 1;
    }
    let significant = 63 - masked.leading_zeros() as usize;
    significant / 6
}

fn slot_for(deadline: u64, level: usize) -> usize{
    ((deadline >> (level * 6)) & SLOT_MASK) as usize 
}


struct Level<T>{
    slots: [Vec<Entry<T>>; SLOTS],
    occupied: u64,
}

pub struct Wheel<T>{
    levels: [Level<T>; LEVELS],
    elapsed: u64,
    next_id: u64,
}

struct Expiration {
    level: usize,
    slot: usize,
    deadline: u64, // START of the slot's range, not an entry's deadline
}

 impl<T> Wheel<T> {
    pub fn new() -> Self{
        Wheel{
            levels: array::from_fn(|_| Level {
                slots: array::from_fn(|_| Vec::new()),
                occupied: 0,
            }),
            elapsed: 0,
            next_id: 0,
        }
    }

    /*old insert
    pub fn insert(&mut self, deadline: u64, payload:T) -> Result<TimerHandle, InsertError>{
        if deadline < self.elapsed{
            return Err(InsertError::Elapsed);
        }
        if deadline >= self.elapsed + 64{
            return Err(InsertError::TooFar);
        }
        let id = self.next_id;
        self.next_id += 1;
        
        let slot = (deadline % 64) as usize; // if the deadline is 10, then the slot here is 36?
        self.slots[slot].push(Entry{id, deadline, payload});
        self.occupied |= //compound biwise OR a |= b same as a = a|b , set bit as 1 if at least one is 1
         1u64 << slot; // position of the bit. exemple with slot as 4: 0000 0000, 
                       //count from left to right, 0-1-2-3-4, 0000 0000 -> 0001 0000
        // the whole stetment is build a mask with 1 at the position of slot (1u64 << slot) and do OR with the mask.
        // the new one has 0000 1000, with |= the only change is on the 4th bit, if it was 0 it becomes 1, if it was
        // 1 it stays 1 becuse 0(new)+1(mask)=1(mask) and 0(new)+0(mask)=0(mask).
        Ok(TimerHandle { id, slot })
    }*/

    fn insert_entry(&mut self, entry: Entry<T>){
        let level = level_for(self.elapsed, entry.deadline);
        let slot = slot_for(entry.deadline, level);

        self.levels[level].occupied |= 1u64 << slot;
        self.levels[level].slots[slot].push(entry);
    }

    pub fn insert(&mut self, deadline: u64, payload: T) -> Result<TimerHandle, InsertError>{
        if deadline < self.elapsed{return Err(InsertError::Elapsed);}
        if deadline > self.elapsed + MAX_DURATION { return Err(InsertError::TooFar);}

        let id = self.next_id;
        self.next_id += 1;

        self.insert_entry(Entry {id, deadline, payload});
        Ok(TimerHandle{id,deadline})
    }

    /* old expire
    pub fn expire(&mut self, now: u64) -> Vec<T>{
        let mut expired = Vec::new();
        if now < self.elapsed {
            return expired;
        }
        for tick in self.elapsed..=now{ // since the last tick until now
            let slot = (tick % 64) as usize;
            
            let drained_values =self.slots[slot].drain(..);
            expired.extend(drained_values.map(|entry| entry.payload));
            self.occupied &= !(1u64 << slot); // similar with the above, but now we want to set the bit to 0
                                              // for this we set at the slot position to 1, the rest is zero, then
                                              // flip everything with !, now we do an &= AND, only 1+1=1 the rest is 0,
                                              // wich means that everything still unch4anged, only the position
                                              // where the 1 needs to be 0 is affected.
        }
        self.elapsed = now + 1; // advance the wheel to the next tick
        expired
    }*/

    pub fn expire(&mut self, now: u64) -> Vec<T>{
        let mut expired = Vec::new();
        if now < self.elapsed{return expired;}

        while let Some(exp) = self.next_expiration(){
            if exp.deadline > now {break;}
            self.elapsed = self.elapsed.max(exp.deadline);

            let drained: Vec<Entry<T>> = self.levels[exp.level].slots[exp.slot].drain(..).collect();
            self.levels[exp.level].occupied &= !(1u64 << exp.slot);

            if exp.level == 0{
                expired.extend(drained.into_iter().map(|entry| entry.payload));
            } else {
                for entry in drained{
                    self.insert_entry(entry);
                }
            }
        }
        self.elapsed = now + 1;
        expired
    }

    /// The oracle: the true earliest deadline, found by looking at every entry
    /// in all 384 slots. `next_deadline` must never report a time *after* this.
    #[cfg(test)]
    fn next_deadline_slow(&self) -> Option<u64>{
        self.levels
            .iter()
            .flat_map(|level| level.slots.iter())
            .flat_map(|slot| slot.iter())
            .map(|entry| entry.deadline)
            .min()
    }

    /// Which level an entry with this deadline is *actually* sitting in, found
    /// by search. Cascading is only observable through something like this.
    #[cfg(test)]
    fn level_of(&self, deadline: u64) -> Option<usize>{
        (0..LEVELS).find(|&level| {
            self.levels[level]
                .slots
                .iter()
                .any(|slot| slot.iter().any(|entry| entry.deadline == deadline))
        })
    }

    #[cfg(test)]
    fn count(&self) -> usize{
        self.levels
            .iter()
            .flat_map(|level| level.slots.iter())
            .map(|slot| slot.len())
            .sum()
    }

    /* old next_deadline
    pub fn next_deadline(&self) -> Option<u64>{
        if self.occupied == 0{
            return None;
        }

        let now_slot = (self.elapsed % 64) as u32;
        let zeros = self.occupied.rotate_right(now_slot).trailing_zeros();

        let slot_index = ((now_slot as u64 + zeros as u64) % 64) as usize;
        debug_assert_eq!(
            self.slots[slot_index].first().map(|e| e.deadline),
            Some(self.elapsed + zeros as u64)
        );
        Some(self.elapsed + zeros as u64)
    }*/

    fn next_expiration(&self) -> Option<Expiration>{
        (0..LEVELS)
        .filter_map(|level| self.levels[level].next_expiration(level, self.elapsed)) 
        .min_by_key(|exp| exp.deadline)
    }

    pub fn next_deadline(&self) -> Option<u64>{
        self.next_expiration().map(|exp| exp.deadline)
    }

    pub fn cancel(&mut self, handle: TimerHandle) -> Option<T>{
        if handle.deadline < self.elapsed{ return None;}

        let level = level_for(self.elapsed, handle.deadline);
        let slot_index = slot_for(handle.deadline, level);
        let slot = &mut self.levels[level].slots[slot_index];

        if let Some(inner_position) = slot.iter().position(|entry| entry.id == handle.id ){
            let entry = slot.remove(inner_position);
            if slot.is_empty(){
                self.levels[level].occupied &= !(1u64 << slot_index); // clear the bit if the slot is empty
            }
            return Some(entry.payload);
        }
        None
    }
 }

impl<T> Level<T>{
    fn next_expiration(&self, level: usize, elapsed: u64) -> Option<Expiration>{
        if self.occupied == 0{ return None;}

        let slot_range = 1u64 << (level * 6);
        let now_slot = (elapsed >> (level * 6)) & SLOT_MASK;

        let distance = self.occupied.rotate_right(now_slot as u32).trailing_zeros() as u64;
        let slot = ((now_slot + distance) & SLOT_MASK) as usize;

        let slot_start = elapsed & !(slot_range - 1); // truncate down to a boundary
        Some(Expiration {level, slot, deadline: slot_start + distance * slot_range})
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64. Deterministic on purpose: the seed is a constant, never the
    /// clock, so a failing differential test reproduces exactly on the next run.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// A number in `0..n`. Panics if `n == 0`.
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    // ------------------------------------------------------------------
    // The math, on its own. No `self`, no wheel, no state. If these fail
    // nothing above them can be trusted, so they run first.
    // ------------------------------------------------------------------

    #[test]
    fn level_for_table(){
        // (elapsed, deadline, expected level)
        let cases = [
            (0u64,        0u64,  0usize),
            (0,           1,     0),
            (0,           63,    0),   // last millisecond of level 0
            (0,           64,    1),   // first that needs a second digit
            (63,          64,    1),   // 1ms apart, still level 1 — see below
            (0,           100,   1),
            (64,          100,   0),   // same deadline, elapsed caught up
            (0,           4_095, 1),
            (0,           4_096, 2),
            (0,         262_143, 2),
            (0,         262_144, 3),
            (0,    MAX_DURATION, 5),
        ];

        for (elapsed, deadline, expected) in cases {
            assert_eq!(
                level_for(elapsed, deadline),
                expected,
                "level_for({elapsed}, {deadline})"
            );
        }
    }

    #[test]
    fn one_millisecond_apart_can_still_be_a_level_apart(){
        // 63 = 0b0111111 and 64 = 0b1000000 disagree in the level-1 digit, so
        // XOR keeps that bit and the entry goes to level 1. Correct, and it
        // costs exactly one cascade at tick 64. Distance is not the question
        // the wheel asks — *which digit disagrees* is.
        assert_eq!(level_for(63, 64), 1);
        assert_eq!(level_for(64, 64), 0);
    }

    #[test]
    fn level_for_never_exceeds_the_top_level(){
        // Without the clamp, this XORs to 37 significant bits and returns 6 —
        // one past the end of `levels`, i.e. an index out of bounds. Delete the
        // clamp and watch this panic.
        assert_eq!(level_for(MAX_DURATION, MAX_DURATION + 1), LEVELS - 1);
    }

    #[test]
    fn slot_for_reads_one_base64_digit(){
        // 5000 = 1*64^2 + 14*64^1 + 8*64^0, i.e. "1 14 8" in base 64.
        assert_eq!(slot_for(5000, 0), 8);
        assert_eq!(slot_for(5000, 1), 14);
        assert_eq!(slot_for(5000, 2), 1);
        assert_eq!(slot_for(5000, 3), 0);
    }

    #[test]
    fn a_deadline_lands_inside_the_slot_that_covers_it(){
        // The pair (level_for, slot_for) has to agree: whatever level is chosen,
        // the slot picked at that level must be the one whose range contains the
        // deadline. This is the property the whole wheel rests on.
        for deadline in [0u64, 1, 63, 64, 100, 4_096, 5_000, 1 << 20, MAX_DURATION] {
            let level = level_for(0, deadline);
            let slot_range = 1u64 << (level * 6);
            let start = slot_for(deadline, level) as u64 * slot_range;

            assert!(
                start <= deadline && deadline < start + slot_range,
                "deadline {deadline} put at L{level} slot {}, whose range is [{start}, {})",
                slot_for(deadline, level),
                start + slot_range
            );
        }
    }

    // ------------------------------------------------------------------
    // Bounds
    // ------------------------------------------------------------------

    #[test]
    fn insert_beyond_max_duration_is_rejected(){
        let mut wheel = Wheel::<()>::new();

        assert!(wheel.insert(MAX_DURATION, ()).is_ok());
        assert_eq!(
            wheel.insert(MAX_DURATION + 1, ()).unwrap_err(),
            InsertError::TooFar
        );
    }

    #[test]
    fn a_deadline_past_the_first_level_is_now_accepted(){
        // This was `TooFar` in step 2. The whole point of levels.
        let mut wheel = Wheel::<char>::new();

        assert!(wheel.insert(64, 'A').is_ok());
        assert_eq!(wheel.level_of(64), Some(1));
    }

    #[test]
    fn insert_in_the_past_is_rejected(){
        let mut wheel = Wheel::<String>::new();
        wheel.expire(10); // advance the wheel to 10
        assert_eq!(wheel.insert(0, "timer".to_string()).unwrap_err(), InsertError::Elapsed);
    }

    #[test]
    fn expire_returns_timers_in_deadline_order(){
        let mut wheel = Wheel::<String>::new();
        wheel.insert(10, "timer1".to_string()).unwrap();
        wheel.insert(5, "timer2".to_string()).unwrap();
        wheel.insert(15, "timer3".to_string()).unwrap();
        let expired = wheel.expire(10);
        assert_eq!(expired, vec!["timer2".to_string(), "timer1".to_string()]);
    }

    #[test]
    fn expire_is_inclusive(){
        let mut wheel = Wheel::<String>::new();
        wheel.insert(10, "timer1".to_string()).unwrap();
        let expired = wheel.expire(10);
        assert_eq!(expired, vec!["timer1".to_string()]);
    }

    #[test]
    fn expire_twice_yields_nothing(){
        let mut wheel = Wheel::<String>::new();
        wheel.insert(10, "timer1".to_string()).unwrap();
        let expired = wheel.expire(10);
        assert_eq!(expired, vec!["timer1".to_string()]);
        let expired2 = wheel.expire(10);
        assert_eq!(expired2, Vec::<String>::new());
    }

    #[test]
    fn expire_backwards_is_a_noop(){
        let mut wheel = Wheel::<String>::new();
        wheel.insert(10, "timer1".to_string()).unwrap();
        let expired = wheel.expire(10);
        assert_eq!(expired, vec!["timer1".to_string()]);
        let expired2 = wheel.expire(5);
        assert_eq!(expired2, Vec::<String>::new());
    }

    #[test]
    fn insert_after_expire_uses_the_new_window(){
        let mut wheel = Wheel::<String>::new();

        wheel.insert(10, "timer1".to_string()).unwrap(); // insert a timer with deadline 10
        wheel.insert(25, "last timer".to_string()).unwrap(); // most late time is 25 times ahead
        let expired = wheel.expire(10); // expire eated the timer 10 times, now the most late is 15 times ahead
        assert_eq!(expired, vec!["timer1".to_string()]);
        
        wheel.insert(11, "middle timmer".to_string()).unwrap(); // put a timer with deadline 10 times ahead and i will use 10 as the new elapsed time again
        let expired3 = wheel.expire(11); // last should still be there
        assert_eq!(expired3, vec!["middle timmer".to_string()]);
    }

    #[test]
    fn cancelled_timer_does_not_fire(){
        let mut wheel = Wheel::<String>::new();
        let handle = wheel.insert(10, "timer1".to_string()).unwrap();
        let cancelled = wheel.cancel(handle);
        assert_eq!(cancelled, Some("timer1".to_string()));
        let expired = wheel.expire(10);
        assert_eq!(expired, Vec::<String>::new());
    }

    #[test]
    fn next_deadline_on_empty_is_none(){
        let wheel = Wheel::<String>::new();
        assert_eq!(wheel.next_deadline(), None);
    }

    #[test]
    fn next_deadline_is_the_minimum(){
        let mut wheel = Wheel::<String>::new();
        wheel.insert(10, "timer1".to_string()).unwrap();
        wheel.insert(5, "timer2".to_string()).unwrap();
        wheel.insert(15, "timer3".to_string()).unwrap();
        assert_eq!(wheel.next_deadline(), Some(5));
    }

    #[test]
    fn next_deadline_slow(){
        let mut wheel = Wheel::<String>::new();
        wheel.insert(10, "timer1".to_string()).unwrap();
        wheel.insert(5, "timer2".to_string()).unwrap();
        wheel.insert(15, "timer3".to_string()).unwrap();
        let expired = wheel.expire(10);
        assert_eq!(expired, vec!["timer2".to_string(), "timer1".to_string()]);
        assert_eq!(wheel.next_deadline(), Some(15));
    }

    // --- the two branches of cancel's conditional bit-clear ---

    #[test]
    fn cancel_one_of_two_keeps_the_slot_occupied() {
        let mut wheel = Wheel::<char>::new();
        let handle_a = wheel.insert(10, 'A').unwrap();
        let _handle_b = wheel.insert(10, 'B').unwrap();

        assert_eq!(wheel.cancel(handle_a), Some('A'));

        // 'B' still lives in slot 10, so bit 10 must still be set. If `cancel`
        // cleared it unconditionally, the fast path reports None and 'B' would
        // never fire.
        assert_eq!(wheel.next_deadline(), Some(10));
        assert_eq!(wheel.next_deadline(), wheel.next_deadline_slow());
        assert_eq!(wheel.expire(10), vec!['B']);
    }

    #[test]
    fn cancel_the_last_entry_clears_the_slot() {
        let mut wheel = Wheel::<char>::new();
        let handle = wheel.insert(10, 'A').unwrap();

        assert_eq!(wheel.cancel(handle), Some('A'));

        // Slot 10 is now empty, so bit 10 must be clear. Leaving it set would
        // make the fast path report a deadline with nothing behind it.
        assert_eq!(wheel.next_deadline(), None);
        assert_eq!(wheel.next_deadline(), wheel.next_deadline_slow());
    }

    // ------------------------------------------------------------------
    // Cascading — the only thing genuinely new in step 3
    // ------------------------------------------------------------------

    #[test]
    fn a_far_timer_descends_one_level_at_a_time(){
        let mut wheel = Wheel::<char>::new();
        wheel.insert(5000, 'A').unwrap();

        // 5000 is "1 14 8" in base 64 -> level 2, slot 1, covering [4096, 8192).
        assert_eq!(wheel.level_of(5000), Some(2));

        // Crossing 4096 empties that level-2 slot. The entry does NOT fire — it
        // is re-inserted at the larger `elapsed`, which buys one more digit.
        assert_eq!(wheel.expire(4096), Vec::<char>::new());
        assert_eq!(wheel.level_of(5000), Some(1));

        // Level 1 slot 14 starts at 4096 + 14*64 = 4992.
        assert_eq!(wheel.expire(4992), Vec::<char>::new());
        assert_eq!(wheel.level_of(5000), Some(0));

        // Now, and only now, is the wheel precise to the millisecond.
        assert_eq!(wheel.expire(4999), Vec::<char>::new());
        assert_eq!(wheel.expire(5000), vec!['A']);
        assert_eq!(wheel.count(), 0);
    }

    #[test]
    fn one_expire_call_can_cascade_and_fire(){
        // The `while` loop in `expire` must keep going after a cascade: the
        // entry it just moved down may now be due within the same call. Replace
        // the loop with a single `if` and this returns [] instead of ['A'].
        let mut wheel = Wheel::<char>::new();
        wheel.insert(5000, 'A').unwrap();

        assert_eq!(wheel.expire(5000), vec!['A']);
    }

    #[test]
    fn cancel_survives_a_cascade(){
        let mut wheel = Wheel::<char>::new();
        let handle = wheel.insert(5000, 'A').unwrap();
        wheel.insert(6000, 'B').unwrap();

        wheel.expire(4096); // both leave level 2
        assert_eq!(wheel.level_of(5000), Some(1));

        // The handle still only knows `deadline: 5000`. It was never told about
        // the move, and does not need to be: the location is recomputed from
        // the current `elapsed`. That is the invariant cascading maintains.
        assert_eq!(wheel.cancel(handle), Some('A'));
        assert_eq!(wheel.expire(10_000), vec!['B']);
    }

    #[test]
    fn next_deadline_may_be_early_but_never_late(){
        let mut wheel = Wheel::<char>::new();
        wheel.insert(5000, 'A').unwrap();

        // The wheel only knows "somewhere in [4096, 8192)" and reports the
        // range start — 904ms early. Early is a wasted wakeup that costs one
        // cascade. Late would be a timer that fires after its deadline.
        assert_eq!(wheel.next_deadline(), Some(4096));
        assert_eq!(wheel.next_deadline_slow(), Some(5000));

        // Once it has cascaded to level 0, the two agree exactly.
        wheel.expire(4992);
        assert_eq!(wheel.next_deadline(), Some(5000));
        assert_eq!(wheel.next_deadline_slow(), Some(5000));
    }

    /// The relationship that must hold after *every* operation, at every level.
    fn assert_consistent<T>(wheel: &Wheel<T>, step: usize){
        let fast = wheel.next_deadline();
        let slow = wheel.next_deadline_slow();

        assert_eq!(
            fast.is_some(),
            slow.is_some(),
            "step {step}: emptiness disagrees, fast={fast:?} slow={slow:?}"
        );

        if let (Some(fast), Some(slow)) = (fast, slow) {
            assert!(
                fast <= slow,
                "step {step}: next_deadline is LATE, {fast} > {slow}"
            );
        }

        // `min_by_key` over the six levels turns out to be redundant: the
        // LOWEST OCCUPIED LEVEL always holds the earliest expiration. If a
        // level-1 slot could expire before level 0's next, the entry in it
        // would agree with `elapsed` on the level-1 digit — and `level_for`
        // would have put it in level 0. Tokio relies on this and returns the
        // first occupied level (wheel/mod.rs:179). We keep the `min` and check
        // the claim here instead of assuming it.
        let lowest_occupied = (0..LEVELS)
            .find_map(|level| wheel.levels[level].next_expiration(level, wheel.elapsed))
            .map(|exp| exp.deadline);

        assert_eq!(
            lowest_occupied, fast,
            "step {step}: the lowest occupied level is not the earliest"
        );
    }

    /// Differential test. The slow version is the oracle — we never assert what
    /// the answer *is*, only that the fast path never overshoots it. Any drift
    /// between `occupied` and `slots`, or a cascade that loses an entry, shows
    /// up here.
    #[test]
    fn next_deadline_never_overshoots_the_oracle(){
        let mut wheel = Wheel::<u64>::new();
        let mut rng = Rng(0x2545F4914F6CDD1D);
        let mut live: Vec<TimerHandle> = Vec::new();

        for step in 0..5000 {
            match rng.below(3) {
                0 => {
                    // INSERT. Mostly near, sometimes far, so entries exist at
                    // several levels simultaneously and cascades overlap.
                    let span = if rng.below(4) == 0 { 100_000 } else { 64 };
                    let deadline = wheel.elapsed + rng.below(span);
                    if let Ok(handle) = wheel.insert(deadline, deadline) {
                        live.push(handle);
                    }
                }
                1 => {
                    // CANCEL. `live` keeps handles for timers that have already
                    // fired, so some of these miss — that exercises the None path.
                    if !live.is_empty() {
                        let i = rng.below(live.len() as u64) as usize;
                        let handle = live.swap_remove(i);
                        wheel.cancel(handle);
                    }
                }
                _ => {
                    // EXPIRE in jumps big enough to cross level boundaries.
                    let now = wheel.elapsed + rng.below(200);
                    wheel.expire(now);
                }
            }

            assert_consistent(&wheel, step);
        }
    }

    // ------------------------------------------------------------------
    // The test that decides whether step 3 is done
    // ------------------------------------------------------------------

    /// 10k random deadlines through both structures, stepping one millisecond
    /// at a time. The heap cannot be wrong about ordering — it is a heap — so
    /// any disagreement is the wheel's.
    #[test]
    fn the_wheel_agrees_with_the_heap(){
        use crate::m1_time::heap::Timers;

        let mut rng = Rng(0x9E3779B97F4A7C15);
        let mut wheel = Wheel::<u64>::new();
        let mut heap: Timers<u64> = Timers::new();

        let n = 10_000usize;
        let mut last = 0u64;

        for _ in 0..n {
            let deadline = rng.below(200_000);
            wheel.insert(deadline, deadline).unwrap();
            heap.insert(deadline, deadline);
            last = last.max(deadline);
        }

        let mut from_wheel = Vec::with_capacity(n);
        let mut from_heap = Vec::with_capacity(n);

        for now in 0..=last {
            from_wheel.extend(wheel.expire(now));
            from_heap.extend(heap.expire(now));
        }

        assert_eq!(from_wheel.len(), n, "the wheel lost or duplicated entries");
        assert_eq!(wheel.count(), 0, "the wheel is still holding something");
        assert!(
            from_wheel.windows(2).all(|w| w[0] <= w[1]),
            "output is not in deadline order"
        );
        assert_eq!(from_wheel, from_heap);
    }

    /// Same, but expiring in random jumps instead of 1ms steps — so a single
    /// `expire` call has to cascade several levels and fire several deadlines
    /// in the right order, rather than one deadline per call.
    #[test]
    fn the_wheel_agrees_with_the_heap_under_jumpy_expire(){
        use crate::m1_time::heap::Timers;

        let mut rng = Rng(0xDEADBEEFCAFEF00D);
        let mut wheel = Wheel::<u64>::new();
        let mut heap: Timers<u64> = Timers::new();

        let n = 10_000usize;
        let mut last = 0u64;

        for _ in 0..n {
            let deadline = rng.below(200_000);
            wheel.insert(deadline, deadline).unwrap();
            heap.insert(deadline, deadline);
            last = last.max(deadline);
        }

        let mut from_wheel = Vec::with_capacity(n);
        let mut from_heap = Vec::with_capacity(n);
        let mut now = 0u64;

        while now <= last {
            from_wheel.extend(wheel.expire(now));
            from_heap.extend(heap.expire(now));
            now += 1 + rng.below(3000);
        }

        from_wheel.extend(wheel.expire(last));
        from_heap.extend(heap.expire(last));

        assert_eq!(from_wheel.len(), n);
        assert_eq!(from_wheel, from_heap);
    }
}


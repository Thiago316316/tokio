 
use std::array;

 struct Entry<T>{
    id: u64,
    deadline: u64,
    payload: T,
 }

 pub struct Wheel<T>{
    slots: [Vec<Entry<T>>; 64],
    elapsed: u64, // = next tick not yet processed
    next_id: u64,
    occupied: u64, // bitmask of occupied slots
 }

#[derive(Debug, PartialEq, Eq)]
pub enum InsertError{
    TooFar,
    Elapsed,
}

 impl<T> Wheel<T> {
    pub fn new() -> Self{
        Wheel{
            slots: array::from_fn(|_| Vec::new()),
            elapsed: 0,
            next_id: 0,
            occupied: 0,
        }
    }

    pub fn insert(&mut self, deadline: u64, payload:T) -> Result<u64, InsertError>{
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
        Ok(id)
    }

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
                                              // wich means that everything still unchanged, only the position
                                              // where the 1 needs to be 0 is affected.
        }
        self.elapsed = now + 1; // advance the wheel to the next tick
        expired
    }

    #[cfg(test)]
    pub fn next_deadline_slow(&self) -> Option<u64>{
        for tick in self.elapsed..self.elapsed + 64{
            let slot = (tick % 64) as usize;
            if let Some(entry) = self.slots[slot].iter().next(){
                return Some(entry.deadline);
            }
        }
        None
    }

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
    }

    pub fn cancel(&mut self, id: u64) -> Option<T>{
        for slot_index in 0..64{
            
            let slot = &mut self.slots[slot_index];
            if let Some(inner_position) = slot.iter().position(|entry| entry.id == id ){
                let entry = slot.remove(inner_position);
                if slot.is_empty(){
                    self.occupied &= !(1u64 << slot_index);
                }
                return Some(entry.payload);
            }
        }
        None
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

    #[test]
    fn insert_at_elapsed_plus_64_is_rejected(){
        let mut wheel = Wheel::<()>::new();
       
        assert_eq!(wheel.insert(64,()).unwrap_err(), InsertError::TooFar);
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
        let id = wheel.insert(10, "timer1".to_string()).unwrap();
        let cancelled = wheel.cancel(id);
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
        let id_a = wheel.insert(10, 'A').unwrap();
        let _id_b = wheel.insert(10, 'B').unwrap();

        assert_eq!(wheel.cancel(id_a), Some('A'));

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
        let id = wheel.insert(10, 'A').unwrap();

        assert_eq!(wheel.cancel(id), Some('A'));

        // Slot 10 is now empty, so bit 10 must be clear. Leaving it set would
        // make the fast path report a deadline with nothing behind it.
        assert_eq!(wheel.next_deadline(), None);
        assert_eq!(wheel.next_deadline(), wheel.next_deadline_slow());
    }

    /// Differential test: `next_deadline` (bitmask) must agree with
    /// `next_deadline_slow` (walks the slots) after every single operation.
    ///
    /// The slow version is the oracle — we never assert what the answer *is*,
    /// only that the two derivations agree. Any drift between `occupied` and
    /// `slots` shows up here.
    #[test]
    fn next_deadline_matches_brute_force() {
        let mut wheel = Wheel::<u64>::new();
        let mut rng = Rng(0x2545F4914F6CDD1D);
        let mut live: Vec<u64> = Vec::new(); // ids we have inserted

        for step in 0..2000 {
            match rng.below(3) {
                0 => {
                    // INSERT. Staying inside [elapsed, elapsed + 64) keeps us
                    // off the TooFar/Elapsed paths, which are tested elsewhere.
                    let deadline = wheel.elapsed + rng.below(64);
                    if let Ok(id) = wheel.insert(deadline, deadline) {
                        live.push(id);
                    }
                }
                1 => {
                    // CANCEL. `live` accumulates stale ids as timers fire, so
                    // some of these are misses — that exercises the None path.
                    if !live.is_empty() {
                        let i = rng.below(live.len() as u64) as usize;
                        let id = live.swap_remove(i);
                        wheel.cancel(id);
                    }
                }
                _ => {
                    // EXPIRE, in small steps so the wheel usually holds several
                    // timers. Large jumps would keep it empty and test nothing.
                    let now = wheel.elapsed + rng.below(8);
                    wheel.expire(now);
                }
            }

            assert_eq!(
                wheel.next_deadline(),
                wheel.next_deadline_slow(),
                "diverged at step {step}"
            );
        }
    }
}


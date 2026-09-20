//! The oracle: a binary heap of `(deadline, id)`.
//!
//! This is step 1 of M1, and it is kept forever — not as a fallback, but as the
//! thing `wheel.rs` is differential-tested against. A heap cannot be wrong
//! about ordering, because ordering is the only thing it is. So when the wheel
//! and the heap disagree, the wheel is wrong, and no further argument is needed.
//!
//! # What it costs
//!
//! `insert` and `expire` are O(log n) — fine. `cancel` is the problem, and
//! feeling that is the entire reason this file exists before `wheel.rs` does.
//!
//! The heap property orders parent-to-child only. There is no arithmetic that
//! takes an id and returns a position, so removing an arbitrary entry means
//! finding it first: O(n). The implementation below is even blunter than that —
//! it drains the whole heap and rebuilds it — because the honest version of
//! "you cannot do this" is better than a clever version that hides it.
//!
//! The wheel's answer is not a faster search. It is to replace searching with
//! addressing: compute where the entry must be from the deadline itself.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

pub struct Timers<T> {
    heap: BinaryHeap<Reverse<(u64, u64)>>,
    payloads: HashMap<u64, T>,
    next_id: u64,
}

impl<T> Timers<T> {
    pub fn new() -> Self {
        Timers {
            heap: BinaryHeap::new(),
            payloads: HashMap::new(),
            next_id: 0,
        }
    }

    pub fn insert(&mut self, time_ms: u64, payload: T) -> u64 {
        let id = self.next_id;
        self.heap.push(Reverse((time_ms, id)));
        self.payloads.insert(id, payload);

        self.next_id += 1;
        id
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.heap.peek().cloned().map(|Reverse((time_ms, _))| time_ms)
    }

    pub fn expire(&mut self, current_time_ms: u64) -> Vec<T> {
        let mut expired = Vec::new();
        while let Some(Reverse((time_ms, id))) = self.heap.peek().copied() {
            if time_ms <= current_time_ms {
                self.heap.pop();
                expired.push(self.payloads.remove(&id).unwrap());
            } else {
                break;
            }
        }
        expired
    }

    pub fn cancel(&mut self, id: u64) -> Option<T> {
        if let Some(payload) = self.payloads.remove(&id) {
            // Remove the entry from the heap by filtering it out
            self.heap = self
                .heap
                .drain()
                .filter(|Reverse((_, heap_id))| *heap_id != id)
                .collect();
            Some(payload)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_timers() -> Timers<char> {
        let mut timers: Timers<char> = Timers::new();
        let _id1 = timers.insert(30, 'C');
        let _id2 = timers.insert(10, 'A');
        let _id3 = timers.insert(20, 'B');
        timers
    }

    #[test]
    fn cancel_timers(){
        let mut timers: Timers<char> = Timers::new();
        let _id1 = timers.insert(30, 'C');
        let _id2 = timers.insert(10, 'A');
        let id3 = timers.insert(20, 'B');
        
        let canceled_payload = timers.cancel(id3);
        eprintln!("Canceled payload: {:?}", canceled_payload); // Debug print to show the canceled payload
        assert_eq!(canceled_payload, Some('B')); // Check that the canceled payload is

        let expired = timers.expire(100);
        eprintln!("Expired timers after cancel: {:?}", expired); // Debug print to show the expired timers after canceling 'B'
        assert_eq!(expired, vec!['A', 'C']); // Check that the expired timers are returned in order of their deadlines, and 'B' is not included
    }

    //basics of timers

    #[test]
    fn expire_returns_in_deadline_order(){
        let mut timers: Timers<char> = setup_timers();
        let expired = timers.expire(100);
        //eprintln!("Reversed heap: {:?}", expired); // Debug print to show the order of expired timers
        assert_eq!(expired, vec!['A', 'B', 'C']); // Check that the expired timers are returned in order of their deadlines
    }

    #[test]
    fn equal_deadlines_are_fifo(){
        let mut timers: Timers<char> = Timers::new();
        let _id1 = timers.insert(10, 'A');
        let _id2 = timers.insert(10, 'B');
        let _id3 = timers.insert(10, 'C');

        let expired = timers.expire(100);
        assert_eq!(expired, vec!['A', 'B', 'C']); // Check that the expired timers are returned in order of their deadlines
    }

    #[test]
    fn expire_is_inclusive(){
        let mut timers: Timers<char> = setup_timers();

        let expire_10_inclusive = timers.expire(10); // needs to be <= 10 (incluindo o 10)
        assert_eq!(expire_10_inclusive, vec!['A']); // Check that the expired timers are returned in order of their deadlines
    }

    #[test]
    fn expire_leaves_future_timers(){
        let mut timers: Timers<char> = setup_timers();

        let expired_only_until_15 = timers.expire(15); // should only expire 'A'
        assert_eq!(expired_only_until_15, vec!['A']); // Check that the

        let expired_lasted_20 = timers.expire(20); // should only expire 'B' becuse the limit is 20 and the previus expire didn't reach 20
        assert_eq!(expired_lasted_20, vec!['B']); // Check that the expired timers are returned in order of their deadlines
    }

    #[test]
    fn expire_twice_yields_nothing(){
        let mut timers: Timers<char> = setup_timers();

        let expired_first_time = timers.expire(100); // should expire all
        assert_eq!(expired_first_time, vec!['A', 'B', 'C']); // Check that the expired timers are returned in order of their deadlines

        let expired_second_time = timers.expire(100); // should yield nothing
        assert_eq!(expired_second_time, vec![]); // Check that the expired timers are returned in order of their deadlines
    }

    #[test]
    fn next_deadline_on_empty_is_none(){
        let timers: Timers<char> = Timers::new();
        assert_eq!(timers.next_deadline(), None);
    }

    #[test]
    fn next_deadline_is_the_minimum(){
        let timers: Timers<char> = setup_timers();

        assert_eq!(timers.next_deadline(), Some(10));
    }

    #[test]
    fn next_deadline_does_not_consume(){
        let mut timers: Timers<char> = setup_timers();

        assert_eq!(timers.next_deadline(), Some(10));
        assert_eq!(timers.next_deadline(), Some(10));
        assert_eq!(timers.next_deadline(), Some(10)); // Check that calling next_deadline again does not consume the timer
    
        timers.expire(25); // Expire first two timers
        assert_eq!(timers.next_deadline(), Some(30)); // Check that next_deadline returns

        timers.expire(35); // Expire the last timer
        assert_eq!(timers.next_deadline(), None); // Check that next_deadline returns None
    }

    #[test]
    fn expire_on_empty_is_empty(){
        let mut timers: Timers<char> = Timers::new();
        let expired = timers.expire(100);
        assert_eq!(expired, vec![]); // Check that expiring on an empty timer returns an empty vector
    }

    #[test]
    fn insert_after_expire_still_works(){
        let mut timers: Timers<char> = setup_timers();

        let expired_first_time = timers.expire(100); // should expire all
        assert_eq!(expired_first_time, vec!['A', 'B', 'C']); // Check that the expired timers are returned in order of their deadlines

        let _id4 = timers.insert(40, 'D');
        assert_eq!(timers.next_deadline(), Some(40)); // Check that next_deadline returns the new timer's deadline

        let expired_second_time = timers.expire(50); // should expire the new timer
        assert_eq!(expired_second_time, vec!['D']); // Check that the expired timer is returned
    }
}

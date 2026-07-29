use std::collections::{BinaryHeap, HashMap};
use std::cmp::Reverse;

    struct Timers<T> {
        heap: BinaryHeap<Reverse<(u64, u64)>>,
        payloads: HashMap<u64, T>, 
        next_id: u64,
    }

    impl<T> Timers<T> {
        
        fn new() -> Self {
            Timers {
                heap: BinaryHeap::new(),
                payloads: HashMap::new(),
                next_id: 0,
            }
        }
        
        fn insert(&mut self, time_ms: u64, payload: T) -> u64 {

            let id = self.next_id;
            self.heap.push(Reverse((time_ms, id)));
            self.payloads.insert(id, payload);
            
            self.next_id += 1;
            id
        }
        
        fn next_deadline(&self) -> Option<u64> {
            self.heap.peek().cloned().map(|Reverse((time_ms, _))| time_ms)
        }

        fn expire(&mut self, current_time_ms: u64) -> Vec<T> {
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

        fn cancel(&mut self, id: u64) -> Option<T>{
            if let Some(payload) = self.payloads.remove(&id) {
                // Remove the entry from the heap by filtering it out
                self.heap = self.heap.drain().filter(|Reverse((_, heap_id))| *heap_id != id).collect();
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
    fn cancel_cost(){
        let n = 10_000;
        let mut t: Timers<u64> = Timers::new();
        let ids: Vec<u64> = (0..n).map(|i| t.insert(i, i)).collect();

        let start = std::time::Instant::now();
        for id in ids { t.cancel(id); }
        eprintln!("n={n} took {:?}", start.elapsed());
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
        let mut timers: Timers<char> = setup_timers();

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


/*
The roadmap for saopaulio

┌───────────┬─────────────────────────────────────────────────────────────────────┬─────────────────────────────┐
│ Milestone │                            What you add                             │         New concept         │
├───────────┼─────────────────────────────────────────────────────────────────────┼─────────────────────────────┤
│ M0        │ block_on + spawn + one run queue + a waker that pushes back         │ task, waker, run queue      │
├───────────┼─────────────────────────────────────────────────────────────────────┼─────────────────────────────┤
│ M1        │ sleep(dur) — needs a deadline store + a park-with-timeout           │ the wheel                   │
├───────────┼─────────────────────────────────────────────────────────────────────┼─────────────────────────────┤
│ M2        │ TCP via epoll — park becomes epoll_wait(timeout = nearest deadline) │ readiness, the driver stack │
├───────────┼─────────────────────────────────────────────────────────────────────┼─────────────────────────────┤
│ M3        │ timeout(dur, fut) — race two futures                                │ combinator, cancellation    │
├───────────┼─────────────────────────────────────────────────────────────────────┼─────────────────────────────┤
│ M4        │ N threads, per-thread queues, stealing                              │ the worker                  │
├───────────┼─────────────────────────────────────────────────────────────────────┼─────────────────────────────┤
│ M5        │ move the wheel from shared to per-worker                            │ the #7467 lesson, earned    │
└───────────┴─────────────────────────────────────────────────────────────────────┴─────────────────────────────┘
 */

 /*
 Step 1 — the heap version (~30 min)
- struct Timers { heap: BinaryHeap<Reverse<(u64, u64)>> } — (deadline, id), plus a HashMap<u64, T> for payloads.
- Implement insert, next_deadline, expire(now).
- Tests: insert out of order, expire returns them in deadline order; next_deadline on empty is None; two timers at the same deadline both fire.
- ✅ Checkpoint: now implement cancel(id). Feel it. Write down in studies/ what you had to give up.

Step 2 — one level, 64 slots (~1 hr)
- slots: [Vec<Entry>; 64], elapsed: u64. Use Vec for now — intrusive lists need unsafe, and you should not be writing unsafe yet.
- insert: reject deadline - elapsed >= 64 with an error. Owning the limitation beats hiding it.
- expire(now): walk elapsed..=now, drain each slot, set elapsed = now.
- Tests: a timer at elapsed + 64 is rejected; expiring past a slot twice yields nothing the second time; expire with now < elapsed doesn't go backwards (see time/mod.rs:305 — tokio hit a real bug here, #3619).
- ✅ Checkpoint: add the occupied: u64 bitfield and make next_deadline use trailing_zeros. Verify against a brute-force scan in a test.

Step 3 — levels and cascading (the hard part, ~half a day)
- fn level_for(&self, deadline: u64) -> usize — from deadline ^ elapsed, find the highest set bit, divide by 6. Work out on paper why XOR is the right operation before you write it; that insight is the whole trick.
- insert picks a level, then a slot within it.
- expire drains a level-0 slot directly, but for level > 0 it re-inserts each entry (which lands it in a lower level). That's cascading — it's re-insertion, not a separate mechanism.
- ✅ Checkpoint — the test that matters: property test. Generate 10k random (deadline, id) pairs, insert them all, then step expire forward one ms at a time to the max deadline, collecting output. Assert the output is exactly the input sorted by deadline. If that passes, your wheel is correct. Compare against your Step-1 heap as the oracle.

Step 4 — only now, plug it into a runtime
- The wheel stores Waker instead of a dummy payload.
- next_deadline() becomes the argument to thread::park_timeout.
- sleep() is a future: on first poll, insert into the wheel and return Pending; on later polls, check whether it fired.
- ✅ You have M1.
 */
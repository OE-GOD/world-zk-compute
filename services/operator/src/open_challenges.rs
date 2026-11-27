//! The challenges the watch loop has seen and not finished handling.

use std::collections::{HashMap, VecDeque};

use alloy::primitives::{Address, B256};

/// A challenge whose dispute task has not been started yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingChallenge {
    pub result_id: B256,
    pub challenger: Address,
}

/// Open challenges, keyed by result id (0x-prefixed hex).
///
/// A challenge is open from the moment the watch loop sees it until its dispute
/// task has finished. Until then it is not recorded as processed, and the saved
/// state points at or before the poll that delivered it (see
/// [`OpenChallenges::resume_block`]), so that a restart sees it again.
#[derive(Debug, Default)]
pub struct OpenChallenges {
    /// For each open challenge the first block of the poll that delivered it.
    poll_blocks: HashMap<String, u64>,
    /// The open challenges without a dispute task, oldest first.
    waiting: VecDeque<WaitingChallenge>,
}

impl OpenChallenges {
    /// Open a challenge delivered by the poll that started at `poll_block` and
    /// queue it for a dispute task. Returns `false`, and changes nothing, if it
    /// is already open.
    pub fn open(&mut self, result_id: B256, challenger: Address, poll_block: u64) -> bool {
        let id = challenge_id(&result_id);
        if self.poll_blocks.contains_key(&id) {
            return false;
        }
        self.poll_blocks.insert(id, poll_block);
        self.waiting.push_back(WaitingChallenge {
            result_id,
            challenger,
        });
        true
    }

    /// Whether the challenge with this id is open.
    pub fn is_open(&self, id: &str) -> bool {
        self.poll_blocks.contains_key(id)
    }

    /// Take the oldest challenge that has no dispute task yet. It stays open.
    pub fn next_waiting(&mut self) -> Option<WaitingChallenge> {
        self.waiting.pop_front()
    }

    /// Number of open challenges without a dispute task.
    pub fn waiting_count(&self) -> usize {
        self.waiting.len()
    }

    /// Close a challenge whose dispute task has finished.
    pub fn close(&mut self, id: &str) {
        self.poll_blocks.remove(id);
    }

    /// Number of open challenges.
    pub fn len(&self) -> usize {
        self.poll_blocks.len()
    }

    /// The block to save as the next one to poll from: `next_block`, or the
    /// start of the earliest poll that delivered a challenge that is still open.
    pub fn resume_block(&self, next_block: u64) -> u64 {
        self.poll_blocks
            .values()
            .copied()
            .fold(next_block, u64::min)
    }
}

/// The id a challenge is tracked under: its result id in 0x-prefixed hex.
pub fn challenge_id(result_id: &B256) -> String {
    format!("0x{}", hex::encode(result_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> String {
        challenge_id(&B256::repeat_byte(byte))
    }

    fn open(challenges: &mut OpenChallenges, byte: u8, poll_block: u64) -> bool {
        challenges.open(
            B256::repeat_byte(byte),
            Address::repeat_byte(byte),
            poll_block,
        )
    }

    #[test]
    fn test_challenge_is_open_until_closed() {
        let mut challenges = OpenChallenges::default();
        assert!(!challenges.is_open(&id(1)));

        assert!(open(&mut challenges, 1, 10));
        assert!(challenges.is_open(&id(1)));

        // Handing it to a task does not close it.
        assert!(challenges.next_waiting().is_some());
        assert!(challenges.is_open(&id(1)));
        assert_eq!(challenges.len(), 1);

        challenges.close(&id(1));
        assert!(!challenges.is_open(&id(1)));
        assert_eq!(challenges.len(), 0);
    }

    #[test]
    fn test_open_challenge_is_not_opened_again() {
        let mut challenges = OpenChallenges::default();
        assert!(open(&mut challenges, 1, 10));
        assert!(!open(&mut challenges, 1, 20));

        assert_eq!(challenges.waiting_count(), 1);
        assert_eq!(challenges.resume_block(30), 10);

        // Not with its task running either.
        challenges.next_waiting();
        assert!(!open(&mut challenges, 1, 20));
        assert_eq!(challenges.waiting_count(), 0);
    }

    #[test]
    fn test_waiting_challenges_are_taken_oldest_first() {
        let mut challenges = OpenChallenges::default();
        for byte in [3, 1, 2] {
            open(&mut challenges, byte, 10);
        }

        let mut taken = Vec::new();
        while let Some(challenge) = challenges.next_waiting() {
            assert_eq!(challenge.result_id[0], challenge.challenger[0]);
            taken.push(challenge.result_id[0]);
        }
        assert_eq!(taken, [3, 1, 2]);
    }

    #[test]
    fn test_resume_block_is_the_earliest_poll_of_an_open_challenge() {
        let mut challenges = OpenChallenges::default();
        assert_eq!(challenges.resume_block(100), 100);

        open(&mut challenges, 1, 40);
        open(&mut challenges, 2, 20);
        open(&mut challenges, 3, 30);
        assert_eq!(challenges.resume_block(100), 20);

        // Running and waiting challenges count alike.
        challenges.next_waiting();
        challenges.close(&id(2));
        assert_eq!(challenges.resume_block(100), 30);
        challenges.close(&id(3));
        assert_eq!(challenges.resume_block(100), 40);
        challenges.close(&id(1));
        assert_eq!(challenges.resume_block(100), 100);

        // Never past the loop's own position.
        open(&mut challenges, 4, 40);
        assert_eq!(challenges.resume_block(35), 35);
    }
}

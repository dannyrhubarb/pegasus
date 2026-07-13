// The deterministic half of Pegasus, extracted so the score backend can
// compile the exact same simulation for server-side replay verification.
// NOTHING in this crate may depend on macroquad, the frame clock, or any
// other source of nondeterminism — see the determinism rules in the game
// repo's CLAUDE.md ("Determinism rules").

pub mod replay;
pub mod sim;
pub mod world;

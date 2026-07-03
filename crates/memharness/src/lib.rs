//! memharness library surface — shared by the binary (`main.rs`) and the hermetic integration
//! tests. DEV-ONLY (see Cargo.toml header); never ships.
#![forbid(unsafe_code)]
pub mod anthropic;
pub mod client;
pub mod corpus;
pub mod daemon;
pub mod frontmatter;
pub mod judge;
pub mod mine;
pub mod ollama;
pub mod resolve;
pub mod stats;
// Module lines are added by their tasks, keeping every intermediate commit compiling:
// pub mod frontmatter;  (Task 4)     pub mod corpus;   (Task 8)
// pub mod ollama;       (Task 12)    pub mod daemon;   (Task 16)
// pub mod client;       (Task 18)    pub mod resolve;  (Task 20)
// pub mod mine;         (Task 22)    pub mod stats;    (Task 24)
// pub mod judge;        (Task 30)    pub mod anthropic;(Task 34)
// pub mod arms;         (Task 36)    pub mod synth;    (Task 38)
// pub mod report;       (Task 40)    pub mod run;      (Task 42)

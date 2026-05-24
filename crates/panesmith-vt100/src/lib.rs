#![doc = include_str!("../README.md")]

pub mod backend;

pub use backend::Vt100Backend;

#[cfg(test)]
mod tests;

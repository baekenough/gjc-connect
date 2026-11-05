//! gjc-connect: Gajae-Code(gjc)를 사내망 모델 서버에 연결하는 설정·실행 도구.
//!
//! 모든 상태는 묶음(번들) 폴더의 `data/` 아래에만 저장하며,
//! 사용자의 `~/.gjc`는 읽거나 쓰지 않는다.

pub mod atomic;
pub mod check;
pub mod commands;
pub mod genconfig;
pub mod launcher;
pub mod platform;
pub mod profile;
pub mod secrets;
pub mod ui;
pub mod urlnorm;

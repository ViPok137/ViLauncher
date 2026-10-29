//! ViLauncher core — бэкенд-модули лаунчера без привязки к GUI-фреймворку.
//!
//!  * [`downloader`] / [`sync`] — асинхронная дельта-загрузка модов и ресурсов
//!  * [`secure_store`] / [`auth`] — токены в системном хранилище ОС
//!  * [`java_runtime`] / [`launch`] — изолированная портативная JRE и запуск игры
//!  * [`installer`] — установка Minecraft + NeoForge
//!  * [`serverping`], [`skins`], [`anticheat`], [`modrinth`], [`discord`] — остальные функции лаунчера

pub mod error;
pub mod fsutil;
pub mod hash;
pub mod downloader;
pub mod sync;
pub mod secure_store;
pub mod auth;
pub mod java_runtime;
pub mod launch;
pub mod installer;
pub mod serverping;
pub mod skins;
pub mod servers;
pub mod instances;
pub mod anticheat;
pub mod modrinth;
pub mod discord;
pub mod gamecfg;
pub mod watchdog;

pub use error::{CoreError, Result};

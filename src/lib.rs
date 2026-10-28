//! Engine-independent core of novabrowser: shared by the CEF edition
//! (`novabrowser`) and the WebKitGTK edition (`novabrowser-webkit`).
//! Nothing in here may depend on a browser engine.
pub mod aria2;
pub mod bindings;
pub mod commands;
pub mod completion;
pub mod config;
pub mod hints;
pub mod history;
pub mod host;
pub mod import;
pub mod keys;
pub mod marks;
pub mod net;
pub mod passwords;
pub mod paths;
pub mod permissions;
pub mod profiles;
pub mod session;
pub mod settings;
pub mod settings_page;
pub mod spawn;
pub mod theme;
pub mod urlmatch;
pub mod welcome;

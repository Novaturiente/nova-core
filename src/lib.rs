//! Engine-independent core of novabrowser: shared by the CEF edition
//! (`novabrowser`) and the WebKitGTK edition (`novabrowser-webkit`).
//! Nothing in here may depend on a browser engine.
pub mod keys;
pub mod paths;
pub mod urlmatch;

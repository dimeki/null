/// The party relay every copy of the app uses unless someone types another address in
/// Settings. It runs `relay/relay.ts` (see relay/README.md); `relay/check.ts` verifies it.
pub const DEFAULT_PARTY_SERVER: &str = "wss://relay.dimeki.cc";

/// Where the app looks for the newest version number. The relay serves this file
/// (see relay/README.md); it can be any https address that returns the same JSON.
pub const UPDATE_MANIFEST_URL: &str = "https://api.github.com/repos/dimeki/null/releases/latest";

use std::error::Error;

/// Version check disabled - Pincelism is an independent fork.
/// This function is kept for compatibility but always returns false (no updates available).
pub fn check_for_updates() -> Result<bool, Box<dyn Error>> {
    // No remote version checking for Pincelism
    Ok(false)
}

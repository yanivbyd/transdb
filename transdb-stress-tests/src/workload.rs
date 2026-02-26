use rand::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Get,
    Put,
    Delete,
}

/// Workload profiles controlling the mix of operations the worker issues.
///
/// | Profile     | GET % | PUT % | DELETE % |
/// |-------------|-------|-------|----------|
/// | ReadHeavy   |   80  |   20  |    0     |
/// | Balanced    |   50  |   45  |    5     |
/// | WriteHeavy  |   20  |   75  |    5     |
/// | PutOnly     |    0  |  100  |    0     |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadProfile {
    ReadHeavy,
    Balanced,
    WriteHeavy,
    PutOnly,
}

impl WorkloadProfile {
    /// Draw a random operation using `rng`.
    pub fn sample(&self, rng: &mut impl Rng) -> Op {
        let roll: u32 = rng.gen_range(0..100);
        self.op_for_roll(roll)
    }

    /// Parse a workload profile from its CLI name (e.g. `"balanced"`).
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "read-heavy" => Some(Self::ReadHeavy),
            "balanced" => Some(Self::Balanced),
            "write-heavy" => Some(Self::WriteHeavy),
            "put-only" => Some(Self::PutOnly),
            _ => None,
        }
    }

    /// Return the canonical CLI name for this profile.
    pub fn as_name(&self) -> &'static str {
        match self {
            Self::ReadHeavy => "read-heavy",
            Self::Balanced => "balanced",
            Self::WriteHeavy => "write-heavy",
            Self::PutOnly => "put-only",
        }
    }

    /// Map a roll in `0..100` to an `Op` according to the profile's percentages.
    /// Exposed for deterministic testing.
    pub fn op_for_roll(&self, roll: u32) -> Op {
        match self {
            WorkloadProfile::ReadHeavy => {
                // GET 80%, PUT 20%
                if roll < 80 { Op::Get } else { Op::Put }
            }
            WorkloadProfile::Balanced => {
                // GET 50%, PUT 45%, DELETE 5%
                if roll < 50 { Op::Get } else if roll < 95 { Op::Put } else { Op::Delete }
            }
            WorkloadProfile::WriteHeavy => {
                // GET 20%, PUT 75%, DELETE 5%
                if roll < 20 { Op::Get } else if roll < 95 { Op::Put } else { Op::Delete }
            }
            WorkloadProfile::PutOnly => Op::Put,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecFamily {
    ProRes,
    DNxHR,
    HEVC,
    H264,
    AV1,
    VP9,
    CinemaDNG,
}

impl CodecFamily {
    pub fn name(&self) -> &'static str {
        match self {
            CodecFamily::ProRes => "ProRes",
            CodecFamily::DNxHR => "DNxHR",
            CodecFamily::HEVC => "HEVC",
            CodecFamily::H264 => "H.264",
            CodecFamily::AV1 => "AV1",
            CodecFamily::VP9 => "VP9",
            CodecFamily::CinemaDNG => "cDNG",
        }
    }

    pub fn all() -> &'static [CodecFamily] {
        &[
            CodecFamily::ProRes,
            CodecFamily::DNxHR,
            CodecFamily::HEVC,
            CodecFamily::H264,
            CodecFamily::AV1,
            CodecFamily::VP9,
            CodecFamily::CinemaDNG,
        ]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProResProfile {
    Proxy,
    LT,
    Standard,
    HQ,
    P4444,
    XQ4444,
}

impl ProResProfile {
    pub fn name(&self) -> &'static str {
        match self {
            ProResProfile::Proxy => "Proxy",
            ProResProfile::LT => "LT",
            ProResProfile::Standard => "Standard",
            ProResProfile::HQ => "HQ",
            ProResProfile::P4444 => "4444",
            ProResProfile::XQ4444 => "4444 XQ",
        }
    }

    pub fn all() -> &'static [ProResProfile] {
        &[
            ProResProfile::Proxy,
            ProResProfile::LT,
            ProResProfile::Standard,
            ProResProfile::HQ,
            ProResProfile::P4444,
            ProResProfile::XQ4444,
        ]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnxhrProfile {
    SQ,
    HD,
    HDX,
    HQX,
    P444,
}

impl DnxhrProfile {
    pub fn name(&self) -> &'static str {
        match self {
            DnxhrProfile::SQ => "SQ",
            DnxhrProfile::HD => "HD",
            DnxhrProfile::HDX => "HDX",
            DnxhrProfile::HQX => "HQX",
            DnxhrProfile::P444 => "444",
        }
    }

    pub fn all() -> &'static [DnxhrProfile] {
        &[
            DnxhrProfile::SQ,
            DnxhrProfile::HD,
            DnxhrProfile::HDX,
            DnxhrProfile::HQX,
            DnxhrProfile::P444,
        ]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HevcProfile {
    Main10_420,
    Main10_444,
}

impl HevcProfile {
    pub fn name(&self) -> &'static str {
        match self {
            HevcProfile::Main10_420 => "Main 10 4:2:0",
            HevcProfile::Main10_444 => "Main 10 4:4:4",
        }
    }

    pub fn is_8bit(&self) -> bool {
        false
    }

    pub fn all() -> &'static [HevcProfile] {
        &[
            HevcProfile::Main10_420,
            HevcProfile::Main10_444,
        ]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Profile {
    Main_8bit,
    High_10bit,
}

impl H264Profile {
    pub fn name(&self) -> &'static str {
        match self {
            H264Profile::Main_8bit => "Main 8-bit",
            H264Profile::High_10bit => "High 10-bit",
        }
    }

    pub fn is_8bit(&self) -> bool {
        matches!(self, H264Profile::Main_8bit)
    }

    pub fn all() -> &'static [H264Profile] {
        &[H264Profile::Main_8bit, H264Profile::High_10bit]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Av1Profile {
    Profile0_420_10bit,
    Profile1_444_10bit,
}

impl Av1Profile {
    pub fn name(&self) -> &'static str {
        match self {
            Av1Profile::Profile0_420_10bit => "Profile 0 4:2:0 10-bit",
            Av1Profile::Profile1_444_10bit => "Profile 1 4:4:4 10-bit",
        }
    }

    pub fn is_8bit(&self) -> bool {
        false
    }

    pub fn all() -> &'static [Av1Profile] {
        &[Av1Profile::Profile0_420_10bit, Av1Profile::Profile1_444_10bit]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vp9Profile {
    Profile2_420_10bit,
    Profile3_444_10bit,
}

impl Vp9Profile {
    pub fn name(&self) -> &'static str {
        match self {
            Vp9Profile::Profile2_420_10bit => "Profile 2 4:2:0 10-bit",
            Vp9Profile::Profile3_444_10bit => "Profile 3 4:4:4 10-bit",
        }
    }

    pub fn is_8bit(&self) -> bool {
        false
    }

    pub fn all() -> &'static [Vp9Profile] {
        &[Vp9Profile::Profile2_420_10bit, Vp9Profile::Profile3_444_10bit]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let pos = all.iter().position(|&x| x == self).unwrap_or(0);
        all[(pos + all.len() - 1) % all.len()]
    }
}

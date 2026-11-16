use libwebrtc::rtp_parameters::RtpCodecCapability;

const OPUS_REQUIRED_PARAMS: [(&str, &str); 3] = [
    ("stereo", "1"),
    ("sprop-stereo", "1"),
    ("maxaveragebitrate", "128000"),
];

pub(super) fn h264_preferences(caps: &[RtpCodecCapability]) -> Vec<RtpCodecCapability> {
    let mut out: Vec<RtpCodecCapability> = caps
        .iter()
        .filter(|cap| {
            cap.mime_type.eq_ignore_ascii_case("video/H264")
                && cap
                    .sdp_fmtp_line
                    .as_deref()
                    .and_then(|fmtp| fmtp_param(fmtp, "packetization-mode"))
                    == Some("1")
        })
        .cloned()
        .collect();
    out.sort_by_key(|cap| !is_constrained_baseline(cap));
    out
}

fn is_constrained_baseline(cap: &RtpCodecCapability) -> bool {
    cap.sdp_fmtp_line
        .as_deref()
        .and_then(|fmtp| fmtp_param(fmtp, "profile-level-id"))
        .is_some_and(|id| id.eq_ignore_ascii_case("42e01f"))
}

fn fmtp_param<'a>(fmtp: &'a str, key: &str) -> Option<&'a str> {
    fmtp.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
    })
}

pub(super) fn munge_offer(sdp: &str) -> String {
    let mut session: Vec<String> = Vec::new();
    let mut media: Vec<Vec<String>> = Vec::new();

    for raw in sdp.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            continue;
        }
        if line.starts_with("m=") {
            media.push(vec![line.to_owned()]);
        } else if let Some(current) = media.last_mut() {
            current.push(line.to_owned());
        } else {
            session.push(line.to_owned());
        }
    }

    let mut changed = false;
    for section in &mut media {
        if munge_section(section) {
            changed = true;
        }
    }

    if !changed && sdp.contains("\r\n") && sdp.ends_with("\r\n") {
        return sdp.to_owned();
    }

    let mut out = String::with_capacity(sdp.len() + 128);
    for line in session {
        out.push_str(&line);
        out.push_str("\r\n");
    }
    for section in media {
        for line in section {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    out
}

fn munge_section(section: &mut Vec<String>) -> bool {
    let opus_pts: Vec<String> = section
        .iter()
        .filter_map(|line| line.strip_prefix("a=rtpmap:"))
        .filter_map(|rest| {
            let (pt, codec) = rest.split_once(' ')?;
            codec
                .trim()
                .eq_ignore_ascii_case("opus/48000/2")
                .then(|| pt.trim().to_owned())
        })
        .collect();

    if opus_pts.is_empty() {
        return false;
    }

    let mut changed = false;
    for pt in opus_pts {
        let fmtp_prefix = format!("a=fmtp:{pt} ");
        let bare_fmtp = format!("a=fmtp:{pt}");
        if let Some(idx) = section
            .iter()
            .position(|line| line.starts_with(&fmtp_prefix) || line == &bare_fmtp)
        {
            let existing = section[idx].strip_prefix(&fmtp_prefix).unwrap_or("").trim();
            let (updated, touched) = ensure_opus_params(existing);
            if touched {
                section[idx] = format!("{fmtp_prefix}{updated}");
                changed = true;
            }
        } else {
            let rtpmap_prefix = format!("a=rtpmap:{pt} ");
            if let Some(rtpmap_idx) = section.iter().position(|l| l.starts_with(&rtpmap_prefix)) {
                let (params, _) = ensure_opus_params("");
                section.insert(rtpmap_idx + 1, format!("{fmtp_prefix}{params}"));
                changed = true;
            }
        }
    }
    changed
}

fn ensure_opus_params(existing: &str) -> (String, bool) {
    let mut parts: Vec<String> = existing
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    let mut changed = false;

    for (key, value) in OPUS_REQUIRED_PARAMS {
        let wanted = format!("{key}={value}");
        let found = parts.iter_mut().find(|part| {
            part.split_once('=')
                .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        });
        match found {
            Some(slot) => {
                if slot.split_once('=').map(|(_, v)| v.trim()) != Some(value) {
                    *slot = wanted;
                    changed = true;
                }
            }
            None => {
                parts.push(wanted);
                changed = true;
            }
        }
    }

    (parts.join(";"), changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(mime_type: &str, fmtp: Option<&str>) -> RtpCodecCapability {
        RtpCodecCapability {
            channels: None,
            clock_rate: Some(90_000),
            mime_type: mime_type.to_owned(),
            sdp_fmtp_line: fmtp.map(str::to_owned),
        }
    }

    #[test]
    fn h264_preferences_filters_and_prioritizes_constrained_baseline() {
        let caps = vec![
            cap("video/VP8", None),
            cap("video/VP9", Some("profile-id=0")),
            cap("video/AV1", None),
            cap("video/H265", None),
            cap(
                "video/H264",
                Some("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=64001f"),
            ),
            cap(
                "video/H264",
                Some("level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42e01f"),
            ),
            cap("video/rtx", Some("apt=96")),
            cap(
                "video/h264",
                Some("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"),
            ),
            cap(
                "video/H264",
                Some("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=4d001f"),
            ),
            cap(
                "video/H264",
                Some("packetization-mode=1;profile-level-id=42E01F"),
            ),
            cap("video/red", None),
            cap("video/ulpfec", None),
        ];

        let prefs = h264_preferences(&caps);
        let fmtps: Vec<&str> = prefs
            .iter()
            .filter_map(|c| c.sdp_fmtp_line.as_deref())
            .collect();

        assert_eq!(
            fmtps,
            vec![
                "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f",
                "packetization-mode=1;profile-level-id=42E01F",
                "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=64001f",
                "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=4d001f",
            ]
        );
    }

    #[test]
    fn h264_preferences_returns_empty_when_no_usable_h264_exists() {
        let caps = vec![
            cap("video/VP8", None),
            cap(
                "video/H264",
                Some("packetization-mode=0;profile-level-id=42e01f"),
            ),
            cap("video/H264", None),
        ];
        assert!(h264_preferences(&caps).is_empty());
    }

    #[test]
    fn munge_offer_appends_missing_opus_params_and_is_idempotent() {
        let sdp = "v=0\r\n\
            m=audio 9 UDP/TLS/RTP/SAVPF 111 63\r\n\
            a=rtpmap:111 opus/48000/2\r\n\
            a=fmtp:111 minptime=10;useinbandfec=1\r\n\
            a=rtpmap:63 red/48000/2\r\n\
            a=fmtp:63 111/111\r\n\
            m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
            a=rtpmap:96 H264/90000\r\n\
            a=fmtp:96 packetization-mode=1;profile-level-id=42e01f\r\n";

        let once = munge_offer(sdp);
        assert!(once.contains(
            "a=fmtp:111 minptime=10;useinbandfec=1;stereo=1;sprop-stereo=1;maxaveragebitrate=128000\r\n"
        ));
        assert!(once.contains("a=fmtp:63 111/111\r\n"));
        assert!(once.contains("a=fmtp:96 packetization-mode=1;profile-level-id=42e01f\r\n"));
        assert!(once.lines().all(|l| !l.ends_with('\r')));
        for chunk in once.split_inclusive('\n') {
            assert!(chunk.ends_with("\r\n"));
        }

        let twice = munge_offer(&once);
        assert_eq!(twice, once);
    }

    #[test]
    fn munge_offer_inserts_fmtp_after_rtpmap_when_missing() {
        let sdp = "v=0\r\n\
            m=audio 9 UDP/TLS/RTP/SAVPF 111 0\r\n\
            a=rtpmap:111 opus/48000/2\r\n\
            a=rtpmap:0 PCMU/8000\r\n\
            a=sendonly\r\n";

        let munged = munge_offer(sdp);
        let expected = "v=0\r\n\
            m=audio 9 UDP/TLS/RTP/SAVPF 111 0\r\n\
            a=rtpmap:111 opus/48000/2\r\n\
            a=fmtp:111 stereo=1;sprop-stereo=1;maxaveragebitrate=128000\r\n\
            a=rtpmap:0 PCMU/8000\r\n\
            a=sendonly\r\n";
        assert_eq!(munged, expected);
    }

    #[test]
    fn munge_offer_leaves_non_opus_and_video_sections_untouched() {
        let sdp = "v=0\r\n\
            m=audio 9 UDP/TLS/RTP/SAVPF 0\r\n\
            a=rtpmap:0 PCMU/8000\r\n\
            m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
            a=rtpmap:96 H264/90000\r\n";
        assert_eq!(munge_offer(sdp), sdp);
    }
}

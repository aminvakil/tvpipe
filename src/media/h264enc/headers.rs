//! Parameter sets and slice headers (Constrained Baseline, CAVLC).
//!
//! Sources: ITU-T H.264 §7.3.2.1.1 (SPS), Annex E.1.1 (VUI), §7.3.2.2
//! (PPS), §7.3.3 (slice header), §7.3.3.1 (ref_pic_list_modification),
//! §7.3.3.3 (dec_ref_pic_marking), Table A-1 (level limits), §A.2.1.1
//! (Constrained Baseline: constraint_set0_flag and constraint_set1_flag),
//! Table E-3..E-5 (colour description codes).

use super::bits::BitWriter;

/// log2(MaxFrameNum).
pub const LOG2_MAX_FRAME_NUM: u32 = 8;

/// Colour description written into the VUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorInfo {
    pub full_range: bool,
    /// colour_primaries, transfer_characteristics, matrix_coefficients
    /// (1 = BT.709, 6 = BT.601-525 / SMPTE 170M).
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
}

impl Default for ColorInfo {
    fn default() -> ColorInfo {
        ColorInfo {
            full_range: false,
            primaries: 1,
            transfer: 1,
            matrix: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeqParams {
    pub width: u32,
    pub height: u32,
    pub mb_w: u32,
    pub mb_h: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub level_idc: u8,
    pub color: ColorInfo,
}

/// (level_idc, MaxMBPS, MaxFS, MaxBR in kbit/s) from Table A-1.
const LEVELS: [(u8, u64, u64, u64); 15] = [
    (10, 1485, 99, 64),
    (11, 3000, 396, 192),
    (12, 6000, 396, 384),
    (13, 11880, 396, 768),
    (20, 11880, 396, 2000),
    (21, 19800, 792, 4000),
    (22, 20250, 1620, 4000),
    (30, 40500, 1620, 10000),
    (31, 108000, 3600, 14000),
    (32, 216000, 5120, 20000),
    (40, 245760, 8192, 20000),
    (41, 245760, 8192, 50000),
    (42, 522240, 8704, 50000),
    (50, 589824, 22080, 135000),
    (51, 983040, 36864, 240000),
];

/// The lowest level whose frame size, macroblock rate and bitrate limits
/// fit (5.2 if none does).
pub fn pick_level(mb_w: u32, mb_h: u32, fps_num: u32, fps_den: u32, kbps: u32) -> u8 {
    let fs = mb_w as u64 * mb_h as u64;
    let mbps = (fs * fps_num as u64).div_ceil(fps_den.max(1) as u64);
    for &(idc, max_mbps, max_fs, max_br) in &LEVELS {
        // Frame dimensions must also fit sqrt(8 · MaxFS) (A.3.1 f/g).
        let max_dim = ((8 * max_fs) as f64).sqrt() as u64;
        if fs <= max_fs
            && mbps <= max_mbps
            && kbps as u64 <= max_br
            && (mb_w as u64) <= max_dim
            && (mb_h as u64) <= max_dim
        {
            return idc;
        }
    }
    52
}

impl SeqParams {
    /// seq_parameter_set_rbsp().
    pub fn sps_rbsp(&self) -> Vec<u8> {
        let mut w = BitWriter::with_capacity(32);
        w.put(8, 66); // profile_idc: Baseline
        w.put(8, 0b1100_0000); // constraint_set0 + set1: Constrained Baseline
        w.put(8, self.level_idc as u32);
        w.ue(0); // seq_parameter_set_id
        w.ue(LOG2_MAX_FRAME_NUM - 4);
        w.ue(2); // pic_order_cnt_type: output order = decoding order
        w.ue(1); // max_num_ref_frames
        w.flag(false); // gaps_in_frame_num_value_allowed_flag
        w.ue(self.mb_w - 1);
        w.ue(self.mb_h - 1);
        w.flag(true); // frame_mbs_only_flag
        w.flag(true); // direct_8x8_inference_flag
        let crop_r = (self.mb_w * 16 - self.width) / 2;
        let crop_b = (self.mb_h * 16 - self.height) / 2;
        let crop = crop_r != 0 || crop_b != 0;
        w.flag(crop);
        if crop {
            w.ue(0);
            w.ue(crop_r);
            w.ue(0);
            w.ue(crop_b);
        }
        w.flag(true); // vui_parameters_present_flag
        self.vui(&mut w);
        w.trailing();
        w.into_bytes()
    }

    fn vui(&self, w: &mut BitWriter) {
        w.flag(true); // aspect_ratio_info_present_flag
        w.put(8, 1); // 1:1
        w.flag(false); // overscan_info_present_flag
        w.flag(true); // video_signal_type_present_flag
        w.put(3, 5); // video_format: unspecified
        w.flag(self.color.full_range);
        w.flag(true); // colour_description_present_flag
        w.put(8, self.color.primaries as u32);
        w.put(8, self.color.transfer as u32);
        w.put(8, self.color.matrix as u32);
        w.flag(false); // chroma_loc_info_present_flag
        w.flag(true); // timing_info_present_flag
        w.put(32, self.fps_den);
        w.put(32, self.fps_num.saturating_mul(2));
        w.flag(false); // fixed_frame_rate_flag (game frames are not)
        w.flag(false); // nal_hrd_parameters_present_flag
        w.flag(false); // vcl_hrd_parameters_present_flag
        w.flag(false); // pic_struct_present_flag
        w.flag(true); // bitstream_restriction_flag
        w.flag(true); // motion_vectors_over_pic_boundaries_flag
        w.ue(0); // max_bytes_per_pic_denom
        w.ue(0); // max_bits_per_mb_denom
        w.ue(11); // log2_max_mv_length_horizontal (±512 px is plenty)
        w.ue(11); // log2_max_mv_length_vertical
        w.ue(0); // max_num_reorder_frames: no reordering, output at once
        w.ue(1); // max_dec_frame_buffering
    }
}

/// pic_parameter_set_rbsp().
pub fn pps_rbsp() -> Vec<u8> {
    let mut w = BitWriter::with_capacity(8);
    w.ue(0); // pic_parameter_set_id
    w.ue(0); // seq_parameter_set_id
    w.flag(false); // entropy_coding_mode_flag: CAVLC
    w.flag(false); // bottom_field_pic_order_in_frame_present_flag
    w.ue(0); // num_slice_groups_minus1
    w.ue(0); // num_ref_idx_l0_default_active_minus1
    w.ue(0); // num_ref_idx_l1_default_active_minus1
    w.flag(false); // weighted_pred_flag
    w.put(2, 0); // weighted_bipred_idc
    w.se(0); // pic_init_qp_minus26
    w.se(0); // pic_init_qs_minus26
    w.se(0); // chroma_qp_index_offset
    w.flag(true); // deblocking_filter_control_present_flag
    w.flag(false); // constrained_intra_pred_flag
    w.flag(false); // redundant_pic_cnt_present_flag
    w.trailing();
    w.into_bytes()
}

/// What a slice header needs.
#[derive(Debug, Clone, Copy)]
pub struct SliceHeader {
    pub first_mb: u32,
    pub idr: bool,
    /// P slice (else I).
    pub p: bool,
    pub frame_num: u32,
    pub idr_pic_id: u32,
    pub qp: u8,
    /// disable_deblocking_filter_idc (0 or 2).
    pub deblock_idc: u32,
}

impl SliceHeader {
    pub fn write(&self, w: &mut BitWriter) {
        w.ue(self.first_mb);
        w.ue(if self.p { 0 } else { 2 });
        w.ue(0); // pic_parameter_set_id
        w.put(LOG2_MAX_FRAME_NUM, self.frame_num);
        if self.idr {
            w.ue(self.idr_pic_id);
        }
        if self.p {
            w.flag(false); // num_ref_idx_active_override_flag
            w.flag(false); // ref_pic_list_modification_flag_l0
        }
        // dec_ref_pic_marking() (every picture is a reference)
        if self.idr {
            w.flag(false); // no_output_of_prior_pics_flag
            w.flag(false); // long_term_reference_flag
        } else {
            w.flag(false); // adaptive_ref_pic_marking_mode_flag
        }
        w.se(self.qp as i32 - 26); // slice_qp_delta
        w.ue(self.deblock_idc);
        if self.deblock_idc != 1 {
            w.se(0); // slice_alpha_c0_offset_div2
            w.se(0); // slice_beta_offset_div2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::h264::parse_sps;
    use crate::media::h264enc::bits::nal_unit;
    use crate::util::ratio::Ratio;

    #[test]
    fn sps_parses_back() {
        let sp = SeqParams {
            width: 1918,
            height: 1080,
            mb_w: 120,
            mb_h: 68,
            fps_num: 60,
            fps_den: 1,
            level_idc: pick_level(120, 68, 60, 1, 20000),
            color: ColorInfo::default(),
        };
        assert_eq!(sp.level_idc, 42);
        let nal = nal_unit(3, 7, &sp.sps_rbsp());
        let s = parse_sps(&nal).unwrap();
        assert_eq!(s.profile_idc, 66);
        assert_eq!(s.constraint_flags, 0xc0);
        assert_eq!((s.width, s.height), (1918, 1080));
        assert_eq!(s.fps, Some(Ratio::new(60, 1)));
        assert_eq!(pick_level(80, 45, 60, 1, 10000), 32);
        assert_eq!(pick_level(120, 68, 30, 1, 10000), 40);
    }
}

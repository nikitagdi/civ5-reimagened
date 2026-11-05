//! Blend state per [`UiBlend`] slot.

use draw_list::UiBlend;

/// The destination alpha is kept in every slot (the target is opaque).
const KEEP_DST_ALPHA: wgpu::BlendComponent = wgpu::BlendComponent {
    src_factor: wgpu::BlendFactor::Zero,
    dst_factor: wgpu::BlendFactor::One,
    operation: wgpu::BlendOperation::Add,
};

/// Fixed-function blend of a slot; the shader outputs non-premultiplied colour.
// SIMPLIFIED(SIMP-uir-3): the values of the exe's table at 0x0922020c are not read. Slot 0 is the
// alpha blend; slot 2 (the exe picks it for MaskTexture) is taken as alpha too, so masks show
// right; slots 1 and 3 are additive and multiplicative placeholders.
pub(crate) fn blend_state(blend: UiBlend) -> wgpu::BlendState {
    let color = match blend {
        UiBlend::Alpha | UiBlend::Slot2 => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        UiBlend::Slot1 => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        UiBlend::Slot3 => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Dst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
    };
    wgpu::BlendState {
        color,
        alpha: KEEP_DST_ALPHA,
    }
}

/// Slots in the pipeline table.
pub(crate) const SLOTS: [UiBlend; 4] = [
    UiBlend::Alpha,
    UiBlend::Slot1,
    UiBlend::Slot2,
    UiBlend::Slot3,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_in_table_order() {
        for (i, b) in SLOTS.iter().enumerate() {
            assert_eq!(b.slot() as usize, i);
        }
    }

    #[test]
    fn alpha_is_source_over_and_keeps_the_destination_alpha() {
        let s = blend_state(UiBlend::Alpha);
        assert_eq!(s.color.src_factor, wgpu::BlendFactor::SrcAlpha);
        assert_eq!(s.color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        assert_eq!(s.alpha.src_factor, wgpu::BlendFactor::Zero);
        assert_eq!(s.alpha.dst_factor, wgpu::BlendFactor::One);
        assert_eq!(blend_state(UiBlend::Slot2), s);
    }
}

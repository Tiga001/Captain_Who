use super::{
    invalid_output, MAX_OFFICE_GRID_COLUMNS, MAX_OFFICE_PAGE_NUMBER,
    MAX_OFFICE_SCREENSHOT_DIMENSION, MAX_OFFICE_TOTAL_PAGES,
};
use crate::office::{
    OfficeEngineError, OfficeGridLayout, OfficePresentationRenderPlan, OfficeRenderGridGeometry,
    OfficeRenderLayoutCoverage, OfficeRenderLayoutEvidence,
};

const GRID_GAP_PX: f64 = 12.0;
const GRID_PADDING_PX: f64 = 12.0;
const GEOMETRY_EPSILON: f64 = 0.01;

/// Stable internal name used by the execution layer. The verification inputs
/// themselves are persisted as part of the approved execution snapshot.
pub(crate) type OfficeRenderLayoutPlan = OfficePresentationRenderPlan;

/// Converts a frozen Host plan into a durable layout-geometry receipt only when
/// the decoded PNG dimensions and every grid bound agree with that plan.
///
/// This function is intentionally fail-closed: page-count diagnostics,
/// `pageSelection`, a successful provider exit, and a valid PNG header are not
/// sufficient inputs and are never consulted here. A successful receipt proves
/// only renderer geometry; it is not per-slide visual-content evidence.
pub(crate) fn verify_render_layout_coverage(
    plan: &OfficeRenderLayoutPlan,
    actual_width: u32,
    actual_height: u32,
) -> Result<OfficeRenderLayoutCoverage, OfficeEngineError> {
    validate_requested_pages(&plan.requested_pages)?;
    if plan.slide_width_emu == 0 || plan.slide_height_emu == 0 {
        return Err(invalid_output(
            "Office render layout verification requires positive frozen slide dimensions.",
        ));
    }
    if plan.viewport.width == 0
        || plan.viewport.height == 0
        || plan.viewport.width > MAX_OFFICE_SCREENSHOT_DIMENSION
        || plan.viewport.height > MAX_OFFICE_SCREENSHOT_DIMENSION
        || actual_width != plan.viewport.width
        || actual_height != plan.viewport.height
    {
        return Err(invalid_output(
            "Office render layout cannot be verified because the PNG dimensions do not match the frozen viewport.",
        ));
    }

    let grid = verify_grid_geometry(plan, actual_width, actual_height)?;
    Ok(OfficeRenderLayoutCoverage {
        requested_pages: plan.requested_pages.clone(),
        evidence: OfficeRenderLayoutEvidence::TrustedRendererGeometry,
        grid,
    })
}

fn verify_grid_geometry(
    plan: &OfficeRenderLayoutPlan,
    actual_width: u32,
    actual_height: u32,
) -> Result<Option<OfficeRenderGridGeometry>, OfficeEngineError> {
    let Some(grid) = plan.grid else {
        if plan.requested_pages.len() != 1 {
            return Err(invalid_output(
                "Office render layout verification requires a frozen grid for multiple slides.",
            ));
        }
        let expected_height = f64::from(actual_width) * f64::from(plan.slide_height_emu)
            / f64::from(plan.slide_width_emu);
        if checked_round_u32(expected_height, "single-slide height")? != actual_height {
            return Err(invalid_output(
                "Office single-slide render dimensions do not match the frozen slide aspect ratio.",
            ));
        }
        return Ok(None);
    };
    let OfficeGridLayout::Columns { columns } = grid else {
        return Err(invalid_output(
            "Office render layout cannot be verified from an unresolved automatic grid.",
        ));
    };
    if columns == 0 || columns > MAX_OFFICE_GRID_COLUMNS {
        return Err(invalid_output(
            "Office render layout verification requires a non-empty frozen grid.",
        ));
    }
    let geometry = derive_grid_geometry(plan, columns, actual_width)?;
    if !approximately_equal(geometry.content_width, f64::from(actual_width))
        || geometry.content_height > f64::from(actual_height) + GEOMETRY_EPSILON
    {
        return Err(invalid_output(
            "Office render layout is invalid because the frozen grid exceeds the PNG bounds.",
        ));
    }

    Ok(Some(OfficeRenderGridGeometry {
        columns,
        rows: geometry.rows,
        viewport_width: plan.viewport.width,
        viewport_height: plan.viewport.height,
        content_width: checked_ceil_u32(geometry.content_width, "content width")?,
        content_height: checked_ceil_u32(geometry.content_height, "content height")?,
    }))
}

#[derive(Debug, Clone, Copy)]
struct GridGeometry {
    rows: u32,
    content_width: f64,
    content_height: f64,
}

fn derive_grid_geometry(
    plan: &OfficeRenderLayoutPlan,
    columns: u16,
    viewport_width: u32,
) -> Result<GridGeometry, OfficeEngineError> {
    let rows = u32::try_from(plan.requested_pages.len().div_ceil(usize::from(columns)))
        .map_err(|_| invalid_output("Office render layout grid exceeds its row limit."))?;
    let columns_f64 = f64::from(columns);
    let rows_f64 = f64::from(rows);
    let horizontal_gaps = f64::from(columns.saturating_sub(1)) * GRID_GAP_PX;
    let available_tile_width = f64::from(viewport_width) - 2.0 * GRID_PADDING_PX - horizontal_gaps;
    if available_tile_width <= 0.0 {
        return Err(invalid_output(
            "Office render layout grid has no positive tile width.",
        ));
    }
    let tile_width = available_tile_width / columns_f64;
    let tile_height =
        tile_width * f64::from(plan.slide_height_emu) / f64::from(plan.slide_width_emu);
    Ok(GridGeometry {
        rows,
        content_width: 2.0 * GRID_PADDING_PX + columns_f64 * tile_width + horizontal_gaps,
        content_height: 2.0 * GRID_PADDING_PX
            + rows_f64 * tile_height
            + f64::from(rows.saturating_sub(1)) * GRID_GAP_PX,
    })
}

fn validate_requested_pages(pages: &[u32]) -> Result<(), OfficeEngineError> {
    if pages.is_empty() || pages.len() > MAX_OFFICE_TOTAL_PAGES as usize {
        return Err(invalid_output(
            "Office render layout verification requires a non-empty bounded page set.",
        ));
    }
    if pages
        .iter()
        .any(|page| *page == 0 || *page > MAX_OFFICE_PAGE_NUMBER)
        || pages.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(invalid_output(
            "Office render layout pages must be positive, sorted, and unique.",
        ));
    }
    Ok(())
}

fn approximately_equal(left: f64, right: f64) -> bool {
    (left - right).abs() <= GEOMETRY_EPSILON
}

fn checked_ceil_u32(value: f64, name: &str) -> Result<u32, OfficeEngineError> {
    let value = value.ceil();
    if !value.is_finite() || value <= 0.0 || value > f64::from(u32::MAX) {
        return Err(invalid_output(format!(
            "Office render layout {name} is outside its supported range."
        )));
    }
    Ok(value as u32)
}

fn checked_round_u32(value: f64, name: &str) -> Result<u32, OfficeEngineError> {
    let value = value.round();
    if !value.is_finite() || value <= 0.0 || value > f64::from(u32::MAX) {
        return Err(invalid_output(format!(
            "Office render layout {name} is outside its supported range."
        )));
    }
    Ok(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::OfficeViewport;

    fn plan(page_count: u32, viewport_height: u32) -> OfficeRenderLayoutPlan {
        OfficeRenderLayoutPlan {
            requested_pages: (1..=page_count).collect(),
            slide_width_emu: 16,
            slide_height_emu: 9,
            viewport: OfficeViewport {
                width: 1600,
                height: viewport_height,
            },
            grid: Some(OfficeGridLayout::Columns { columns: 2 }),
        }
    }

    #[test]
    fn emits_geometry_evidence_when_the_full_grid_fits() {
        let mut plan = plan(9, 2300);
        let geometry = derive_grid_geometry(&plan, 2, plan.viewport.width).unwrap();
        plan.viewport.height = geometry.content_height.ceil() as u32;

        let receipt =
            verify_render_layout_coverage(&plan, plan.viewport.width, plan.viewport.height)
                .unwrap();

        assert_eq!(receipt.requested_pages, (1..=9).collect::<Vec<_>>());
        assert_eq!(
            receipt.evidence,
            OfficeRenderLayoutEvidence::TrustedRendererGeometry
        );
        assert_eq!(receipt.grid.as_ref().unwrap().rows, 5);
    }

    #[test]
    fn rejects_the_clipped_nine_slide_fixed_viewport() {
        let plan = plan(9, 1200);

        let error = verify_render_layout_coverage(&plan, 1600, 1200).unwrap_err();

        assert_eq!(
            error.code(),
            crate::office::OfficeEngineErrorCode::InvalidOutput
        );
        assert!(error.message().contains("exceeds the PNG bounds"));
    }

    #[test]
    fn proves_a_single_slide_without_a_contact_sheet_grid() {
        let mut plan = plan(1, 900);
        plan.grid = None;

        let receipt = verify_render_layout_coverage(&plan, 1600, 900).unwrap();

        assert_eq!(
            receipt.evidence,
            OfficeRenderLayoutEvidence::TrustedRendererGeometry
        );
        assert!(receipt.grid.is_none());
    }

    #[test]
    fn rejects_actual_dimensions_that_differ_from_the_frozen_viewport() {
        let mut plan = plan(1, 900);
        plan.grid = None;

        let error = verify_render_layout_coverage(&plan, 1600, 899).unwrap_err();

        assert!(error.message().contains("do not match the frozen viewport"));
    }

    #[test]
    fn rejects_noncanonical_page_sets_and_unresolved_grid() {
        let mut duplicate = plan(3, 1200);
        duplicate.requested_pages = vec![1, 2, 2];
        assert!(verify_render_layout_coverage(&duplicate, 1600, 1200).is_err());

        let mut unresolved_grid = plan(3, 1200);
        unresolved_grid.grid = Some(OfficeGridLayout::Auto);
        assert!(verify_render_layout_coverage(&unresolved_grid, 1600, 1200).is_err());
    }
}

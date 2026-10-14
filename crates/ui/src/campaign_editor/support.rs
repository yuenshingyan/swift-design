use super::*;

/// The href of the HTML or PDF export: the whole campaign, or one
/// zero-based ad through the export route's `?ad=N` query.
pub(super) fn export_href(campaign_id: &str, extension: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!(
            "/campaigns/{campaign_id}/export{extension}?ad={}",
            index + 1
        ),
        None => format!("/campaigns/{campaign_id}/export{extension}"),
    }
}

/// The href of the PNG export: one ad's image when scoped, the zip
/// of every ad otherwise.
pub(super) fn png_export_href(campaign_id: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/campaigns/{campaign_id}/ads/{}.png", index + 1),
        None => format!("/campaigns/{campaign_id}/export.zip"),
    }
}

/// The download name of a scoped export file, matching the zip entry.
pub(super) fn ad_download_name(campaign_id: &str, index: usize, extension: &str) -> String {
    format!("{campaign_id}-ad-{}.{extension}", index + 1)
}

/// The campaign toolbar's export menus: `All` downloads the whole
/// campaign in one format, and with more than one ad a second menu
/// downloads the ad on screen. Every action is one shot, with no
/// mode to forget.
#[component]
pub(super) fn CampaignExportGroup(
    campaign_id: String,
    selected: usize,
    ad_count: usize,
    can_export_with_chrome: bool,
) -> Element {
    let mut is_all_menu_open = use_signal(|| false);
    let mut is_unit_menu_open = use_signal(|| false);
    let number = selected + 1;
    rsx! {
        div { class: "unit-export",
            button {
                title: "Download the whole campaign",
                onclick: move |_| is_all_menu_open.set(!is_all_menu_open()),
                span { dangerous_inner_html: icons::DOWNLOAD }
                "All"
            }
            if is_all_menu_open() {
                div {
                    class: "menu-backdrop",
                    onclick: move |_| is_all_menu_open.set(false),
                }
                div { class: "toolbar-menu unit-export-menu",
                    a {
                        href: export_href(&campaign_id, "", None),
                        onclick: move |_| is_all_menu_open.set(false),
                        "Download as HTML"
                    }
                    if can_export_with_chrome {
                        a {
                            href: export_href(&campaign_id, ".pdf", None),
                            onclick: move |_| is_all_menu_open.set(false),
                            "Download as PDF"
                        }
                        a {
                            href: png_export_href(&campaign_id, None),
                            onclick: move |_| is_all_menu_open.set(false),
                            "Download as PNG zip"
                        }
                    } else {
                        a {
                            "aria-disabled": "true",
                            title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                            "Download as PDF"
                        }
                        a {
                            "aria-disabled": "true",
                            title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                            "Download as PNG zip"
                        }
                    }
                }
            }
        }
        if ad_count > 1 {
            div { class: "unit-export",
                button {
                    title: "Download only the ad on screen",
                    onclick: move |_| is_unit_menu_open.set(!is_unit_menu_open()),
                    span { dangerous_inner_html: icons::DOWNLOAD }
                    "Ad {number}"
                }
                if is_unit_menu_open() {
                    div {
                        class: "menu-backdrop",
                        onclick: move |_| is_unit_menu_open.set(false),
                    }
                    div { class: "toolbar-menu unit-export-menu",
                        a {
                            href: export_href(&campaign_id, "", Some(selected)),
                            onclick: move |_| is_unit_menu_open.set(false),
                            "Download as HTML"
                        }
                        if can_export_with_chrome {
                            a {
                                href: export_href(&campaign_id, ".pdf", Some(selected)),
                                onclick: move |_| is_unit_menu_open.set(false),
                                "Download as PDF"
                            }
                            a {
                                href: png_export_href(&campaign_id, Some(selected)),
                                download: ad_download_name(&campaign_id, selected, "png"),
                                onclick: move |_| is_unit_menu_open.set(false),
                                "Download as PNG"
                            }
                        } else {
                            a {
                                "aria-disabled": "true",
                                title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                                "Download as PDF"
                            }
                            a {
                                "aria-disabled": "true",
                                title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                                "Download as PNG"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The stage class for an ad of `viewport`: a narrow canvas is
/// limited by height, a wide one fills the width. No size gets the
/// phone bezel.
pub(super) fn preview_stage_class(viewport: design_model::Viewport) -> &'static str {
    if is_narrow_canvas(viewport) {
        return "preview-stage narrow";
    }
    "preview-stage"
}

/// The strip tile width for an ad of `viewport`, in rem. The tile is
/// `STRIP_TILE_HEIGHT_REM` tall at the canvas ratio, but a wide unit
/// such as the 728 by 90 leaderboard would grow past the strip, so the
/// width is capped and the tile shrinks below the shared height.
pub(super) fn strip_tile_width(viewport: design_model::Viewport) -> String {
    let ratio = f64::from(viewport.width) / f64::from(viewport.height);
    let height = STRIP_TILE_HEIGHT_REM.min(TILE_WIDTH_CAP_REM / ratio);
    frame_width_rem(viewport, height)
}

/// Widest a strip tile may be, in rem.
const TILE_WIDTH_CAP_REM: f64 = 18.0;

/// Removes ad `index`, unless it is the last ad: a campaign keeps
/// at least one ad.
pub(super) fn remove_ad(campaign: &mut Campaign, index: usize) {
    if campaign.ads.len() > 1 && index < campaign.ads.len() {
        campaign.ads.remove(index);
    }
}

/// The size option text: the name and the px canvas.
pub(super) fn size_option_label(size: AdSize) -> String {
    let viewport = size.viewport();
    format!(
        "{} · {} × {}",
        size.label(),
        viewport.width,
        viewport.height
    )
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the ad, else `Ad N`.
pub(super) fn ad_label(index: usize, ad: &Ad) -> String {
    fragment_label("Ad", index, &ad.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Ad 5` when the outline has no entry there.
pub(super) fn outline_title(campaign: &Campaign, index: usize) -> String {
    match campaign.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Ad {}", index + 1, index + 1),
    }
}

/// A sample ad for tests: a heading and a paragraph.
#[cfg(test)]
pub(super) fn default_ad() -> Ad {
    Ad {
        html: "<div class='body'><h2>New ad</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 72px; height: 100%; display: flex; flex-direction: column; gap: 16px; } h2 { font-size: 48px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the campaign. Matches the server's
/// provenance paths: absent optional fields are not counted.
pub(super) fn field_count(campaign: &Campaign) -> usize {
    // Campaign title, size, theme name, four colors, three fonts.
    let mut count = 10;
    for ad in &campaign.ads {
        count += 1 + usize::from(ad.css.is_some()) + usize::from(ad.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{Ad, AdSize, Campaign, FontSet, Palette, Theme};

    use super::{
        CampaignPreviewMode, ad_download_name, ad_label, default_ad, export_href, field_count,
        outline_title, png_export_href, preview_stage_class, remove_ad, size_option_label,
        strip_tile_width,
    };

    #[test]
    fn a_scoped_export_names_the_ad_in_href_and_filename() {
        assert_eq!(export_href("launch", "", None), "/campaigns/launch/export");
        assert_eq!(
            export_href("launch", ".pdf", Some(1)),
            "/campaigns/launch/export.pdf?ad=2"
        );
        assert_eq!(
            png_export_href("launch", None),
            "/campaigns/launch/export.zip"
        );
        assert_eq!(
            png_export_href("launch", Some(1)),
            "/campaigns/launch/ads/2.png"
        );
        assert_eq!(ad_download_name("launch", 1, "png"), "launch-ad-2.png");
    }

    #[test]
    fn a_campaign_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(CampaignPreviewMode::ALL[0], CampaignPreviewMode::Read);
        assert_eq!(
            CampaignPreviewMode::ALL.map(CampaignPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(CampaignPreviewMode::Read.render_query(), "");
        assert_eq!(CampaignPreviewMode::Edit.render_query(), "&editable=true");
        assert!(CampaignPreviewMode::Read.title().contains("reader"));
        assert!(CampaignPreviewMode::Edit.title().contains("select"));
    }

    fn campaign() -> Campaign {
        Campaign {
            title: "T".to_owned(),
            theme: Theme {
                name: "n".to_owned(),
                colors: Palette {
                    background: "#ffffff".to_owned(),
                    text: "#1a1d21".to_owned(),
                    accent: "#0e6e63".to_owned(),
                    muted: "#888888".to_owned(),
                },
                fonts: FontSet {
                    heading: "Inter".to_owned(),
                    body: "Inter".to_owned(),
                    mono: "Menlo".to_owned(),
                },
            },
            size: AdSize::MediumRectangle,
            ads: vec![
                Ad {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Link: https://swift.design.".to_owned()),
                },
                default_ad(),
            ],
            outline: Vec::new(),
        }
    }

    #[test]
    fn ad_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(ad_label(0, &default_ad()), "1. New ad");
        let campaign = campaign();
        assert_eq!(ad_label(0, &campaign.ads[0]), "1. Swift Design");
        let empty = Ad {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(ad_label(1, &empty), "2. Ad 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_ad_number() {
        let mut planned = campaign();
        planned.outline = vec!["Front".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Front");
        assert_eq!(outline_title(&planned, 1), "2. Ad 2");
        assert_eq!(outline_title(&planned, 5), "6. Ad 6");
    }

    #[test]
    fn the_default_ad_validates_inside_a_campaign() {
        let campaign = campaign();
        assert_eq!(campaign.validate(), Vec::new());
        // 10 campaign and theme fields, html and notes on the first
        // ad, html and css on the second.
        assert_eq!(field_count(&campaign), 10 + 2 + 2);
    }

    #[test]
    fn a_campaign_keeps_its_last_ad() {
        let mut campaign = campaign();
        remove_ad(&mut campaign, 5);
        assert_eq!(campaign.ads.len(), 2);
        remove_ad(&mut campaign, 0);
        assert_eq!(campaign.ads.len(), 1);
        assert_eq!(campaign.ads[0].html, default_ad().html);
        remove_ad(&mut campaign, 0);
        assert_eq!(campaign.ads.len(), 1);
    }

    #[test]
    fn size_options_name_the_size_and_its_canvas() {
        assert_eq!(
            size_option_label(AdSize::MediumRectangle),
            "Medium rectangle · 300 × 250"
        );
        assert_eq!(
            size_option_label(AdSize::Leaderboard),
            "Leaderboard · 728 × 90"
        );
        assert_eq!(
            size_option_label(AdSize::MobileBanner),
            "Mobile banner · 320 × 100"
        );
    }

    #[test]
    fn tall_units_are_limited_by_height_and_wide_units_by_width() {
        assert_eq!(
            preview_stage_class(AdSize::MediumRectangle.viewport()),
            "preview-stage narrow"
        );
        assert_eq!(
            preview_stage_class(AdSize::HalfPage.viewport()),
            "preview-stage narrow"
        );
        assert_eq!(
            preview_stage_class(AdSize::Skyscraper.viewport()),
            "preview-stage narrow"
        );
        assert_eq!(
            preview_stage_class(AdSize::Leaderboard.viewport()),
            "preview-stage"
        );
        assert_eq!(
            preview_stage_class(AdSize::MobileBanner.viewport()),
            "preview-stage"
        );
    }

    #[test]
    fn wide_units_get_a_capped_strip_tile() {
        // A medium rectangle keeps the shared tile height.
        assert_eq!(strip_tile_width(AdSize::MediumRectangle.viewport()), "6.60");
        // A leaderboard at the shared height would be 44.5 rem wide;
        // the cap holds it to 18 rem.
        assert_eq!(strip_tile_width(AdSize::Leaderboard.viewport()), "18.00");
        assert_eq!(strip_tile_width(AdSize::MobileBanner.viewport()), "17.60");
    }
}

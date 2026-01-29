//! Widget for clipping content to a specific area with offset support
use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

/// A widget that clips content to a specific area with vertical offset
pub struct ClippedRender<'a, W: Widget> {
    widget: W,
    /// The vertical offset into the content to start rendering from
    vertical_offset: u16,
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<W: Widget> ClippedRender<'_, W> {
    pub fn new(widget: W, vertical_offset: u16) -> Self {
        Self {
            widget,
            vertical_offset,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<W: Widget> Widget for ClippedRender<'_, W> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.vertical_offset == 0 {
            // No offset, render normally
            self.widget.render(area, buf);
            return;
        }

        // Create a temporary buffer that's larger than the visible area
        let full_height = area.height + self.vertical_offset;
        let full_area = Rect {
            x: 0,
            y: 0,
            width: area.width,
            height: full_height,
        };

        let mut temp_buf = Buffer::empty(full_area);

        // Render the widget into the temporary buffer
        self.widget.render(full_area, &mut temp_buf);

        // Copy only the visible portion to the actual buffer
        for y in 0..area.height {
            for x in 0..area.width {
                let src_y = y + self.vertical_offset;
                if src_y < full_height {
                    if let Some(src_cell) = temp_buf.cell((x, src_y)) {
                        let dst_x = area.x + x;
                        let dst_y = area.y + y;
                        if dst_x < buf.area.width && dst_y < buf.area.height {
                            if let Some(dst_cell) = buf.cell_mut((dst_x, dst_y)) {
                                *dst_cell = src_cell.clone();
                            }
                        }
                    }
                }
            }
        }
    }
}

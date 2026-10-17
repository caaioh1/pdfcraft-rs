# Delete a box (a shaded or framed rectangle)

Word and LibreOffice draw a shaded, bordered note as plain path painting in the page content:
one filled rectangle and, often, four stroked lines (twice). Deleting the text inside such a note
left the empty frame behind, because Edit a PDF only knew paragraphs, pictures and Form artwork.

In **Edit a PDF → Edit text & images**, point at a box: a faint outline shows what a right-click
will take. **Right-click** it:

- **Delete box** removes the fill and the border and nothing else. The text inside stays.
- **Delete box and its text** also removes the lines of text that lie wholly inside the box. A
  line that is only partly inside is left alone, so a heading just below a note is not taken
  with it, however the paragraphs were grouped.

Each is **one undo step** and is refused, changing nothing, if the page cannot be rewritten
cleanly. A right-click inside a box opens this menu instead of a picture's, even when a picture
lies behind the box (a watermark, for instance).

## What counts as a box

Found from the painting operators themselves:

- a filled rectangle, a stroked rectangle, or both (`re f`, `re S`, `re B`, or the same four
  corners written as `m l l l l h f*`);
- the straight horizontal and vertical lines stroked along its sides (and a duplicate rectangle
  drawn again at the same place) belong to it, so the whole frame goes together.

Not boxes: shapes with curves or a clipping path, rotated shapes, rules thinner than 3 points,
a rectangle that covers the page (its background) and anything drawn inside a Form XObject.
The yellow highlights behind a placeholder are boxes too.

Only the operators of that box are removed. The rest of the content stream is copied byte for
byte, so text, pictures and the graphics state are untouched.

## Engine, CLI and MCP

`Document::drawn_boxes(page)` lists the boxes (user space, reading order, zero-based).
`Edit::DeleteDrawnBox { page, index, with_text }` deletes one through `Session`, with the usual
permission check, history and incremental save.

Both frontends have the same two tools:

```json
[
  {"tool":"doc_open","args":{"path":"input.pdf"}},
  {"tool":"box_list","args":{"doc":1,"page":1}},
  {"tool":"box_delete","args":{"doc":1,"page":1,"index":2,"with_text":true}},
  {"tool":"doc_save","args":{"doc":1,"path":"output.pdf"}}
]
```

`box_list` returns a `generation`, and each box has a 1-based `index`, a displayed `rect`,
`filled`, `stroked`, `fill` (`#rrggbb` when the page sets it) and `operators`. Pass `generation`
to `box_delete` to refuse a stale reference. Indexes change when the page content changes.

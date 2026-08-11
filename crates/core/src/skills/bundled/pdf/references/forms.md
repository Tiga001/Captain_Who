# Fillable forms

Visual appearance and logical form data are separate correctness surfaces. A widget can look correct while the canonical field tree is stale, and a correct `/V` can lack a usable appearance.

## Inspect before filling

1. Preserve the original; determine whether the PDF is signed or encrypted.
2. Enumerate `PdfReader.get_fields()` and every page's `/Widget` annotations.
3. Follow `/Parent` and `/Kids`; record effective names, `/FT`, `/V`, flags, page locations, and `/AP` state.
4. Compare widgets with the canonical `/AcroForm/Fields` tree. If unrelated objects share a name, stop and report ambiguity or make a user-approved static output. Do not call `reattach_fields()` blindly.

Use bounded quoted-heredoc Python for inspection or filling and print only field names, page numbers, status, and errors needed for the decision. Use `reattach_fields()` only for confirmed orphan widgets, then re-enumerate the tree.

## Preserve interaction

Keep the result interactive unless the user asks for a flattened copy. Clone the source, update values according to field type, and use `auto_regenerate=False`. After writing, reopen and verify:

- every requested field exists with the expected canonical `/V`;
- each widget has the same effective value, directly or through `/Parent`;
- updated widgets have non-empty `/AP` `/N` appearances;
- required unchanged fields, actions, and annotations remain;
- rendered affected pages show correct unclipped values and states.

Do not treat `/NeedAppearances`, a successful write, or a good-looking image alone as proof.

## Flatten only when requested

Create a separate flattened PDF. Paint current appearances, remove all `/Widget` annotations and the `/AcroForm` tree, reopen, assert both are absent, then render and inspect every affected page. Warn that editing or flattening may invalidate signatures, and keep an interactive copy when future revision is likely.

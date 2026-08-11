# Fillable forms

Visual appearance and logical form data are separate correctness surfaces. A widget can display a value while the canonical field tree is missing or stale, and a correct `/V` can exist without a usable appearance stream.

## Inspect before filling

1. Preserve the original and determine whether the PDF is signed or encrypted.
2. Enumerate the canonical `/AcroForm/Fields` tree with `pypdf` `get_fields()`.
3. Enumerate every page's `/Widget` annotations, following `/Parent` and `/Kids` relationships.
4. Record each effective field name, `/FT` type, current `/V`, flags, page/widget location, and `/AP` appearance state.
5. Compare the canonical tree with widgets. If duplicate names refer to unrelated objects, stop and report the ambiguity or produce a user-approved static result. Do not call `reattach_fields()` blindly.

Use `reattach_fields()` only for genuinely orphaned widgets after confirming that it will not create duplicate top-level names. Re-enumerate the field tree afterward and require every requested field to exist.

## Fill and preserve interaction

Keep the result interactive unless the user explicitly requests a flattened copy. Clone the source into a writer, update values with the correct type, and use `auto_regenerate=False` so correctness does not depend on a viewer repairing appearances. Handle checkbox, radio, choice, and text values according to their field types and valid appearance states.

After writing, reopen the output and verify all of the following:

- Every expected field exists in the canonical tree with the expected `/V`.
- Every corresponding widget has the same effective value, directly or through `/Parent`.
- Every updated widget has a non-empty normal appearance at `/AP` `/N`.
- Unchanged fields, actions, and page annotations required by the source remain present.
- Rendered pages show the intended value without clipping, stale text, wrong check states, or font substitution.

Do not treat `/NeedAppearances`, a successful write, or a correct-looking PNG alone as proof that the form data is correct.

## Flatten only when requested

Create a separate flattened output. Paint current appearances before removing interactivity. Reopen it and require zero `/Widget` annotations and no remaining `/AcroForm` field tree, then render and inspect every affected page. Be aware that a library's `flatten` option may paint appearances without removing widgets; verify the actual result rather than trusting the option name.

Do not modify or flatten a signed form without warning that the operation may invalidate signatures. Keep an editable copy when future revision is likely.

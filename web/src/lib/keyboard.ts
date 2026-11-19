/**
 * Is this keydown still part of an IME composition (its commit key must not run
 * a command)?
 *
 * Japanese, Chinese and Korean input commit a candidate with Enter, walk the
 * candidate list with the arrows and cancel with Escape. Every handler in the
 * app that binds those keys — the composer's send, the palette filter, the model
 * picker's filter, the session rename field — would act on half a word if it
 * took them.
 *
 * Both spellings of "the IME owns this key" are checked, because the engines
 * disagree and neither one alone is enough:
 *
 * * `event.isComposing` is the standard flag, and it describes the *composition*
 *   rather than the key that ends it. Safari fires the committing Enter with
 *   `isComposing` already false, so a guard that trusted the flag alone would
 *   send the message on the very key press that finished the word.
 * * `keyCode === 229` is the legacy spelling Gecko and Blink use for a key the
 *   IME consumed; Firefox reports the commit that way with `isComposing` false
 *   on several platforms, which is the case the flag misses.
 *
 * The predicate only answers the question. Each caller decides whether to
 * `preventDefault()`: Enter is the one key here whose default is a submit (the
 * browser's implicit form submission), so a guard that claims *every*
 * composition keydown can stop the IME's own text from arriving, while the
 * arrows and Escape have no default worth claiming.
 */
export function isImeComposing(event: KeyboardEvent): boolean {
  return event.isComposing || event.keyCode === 229;
}
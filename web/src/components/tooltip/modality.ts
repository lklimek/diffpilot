// Input modality shared by tooltips and popovers: whether the last input was the keyboard (what `:focus-visible`
// decides in browsers; jsdom cannot tell). Starts true: focus set by a script follows the keyboard.
let keyboard = true;
/** Whether the last input was the keyboard (a focus then shows a popover; Hide then moves the focus). */
export const keyboardModality = () => keyboard;

/** Test-only: forgets the last pointer input, as at page load. */
export const resetKeyboardModality = () => {
  keyboard = true;
};
if (typeof document !== "undefined") {
  document.addEventListener(
    "keydown",
    (event) => {
      if (!["Shift", "Control", "Alt", "Meta"].includes(event.key)) keyboard = true;
    },
    true,
  );
  for (const type of ["pointerdown", "mousedown", "touchstart"]) {
    document.addEventListener(type, () => (keyboard = false), true);
  }
}

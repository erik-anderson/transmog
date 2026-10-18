/** Compares authored values, including invalid input. Callers exclude UI/test/activation state.
 * Capture a baseline only after loading or successfully persisting a draft. */
export class DraftBaseline {
  private baseline = '';
  capture(value: unknown): void { this.baseline = JSON.stringify(value); }
  differs(value: unknown): boolean { return this.baseline !== JSON.stringify(value); }
}

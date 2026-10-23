import { WebUIElement, observable } from "@microsoft/webui-framework";

/** Shared presentation; the owning editor determines equality and persistence. */
export class DraftStatus extends WebUIElement {
  @observable dirty = false;
  @observable busy = false;
  @observable newDraft = false;
  @observable failed = false;
}
DraftStatus.define("draft-status");

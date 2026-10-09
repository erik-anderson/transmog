import { WebUIElement, attr, observable } from '@microsoft/webui-framework';

export class ProxyToggle extends WebUIElement {
  @attr lifecycle = 'stopped';
  @attr pending = '';
  @attr({mode:'boolean'}) ready = false;
  @observable actionLabel = 'Start proxy';
  lifecycleChanged(): void { this.updateLabel(); }
  pendingChanged(): void { this.updateLabel(); }
  private updateLabel(): void {
    this.actionLabel = this.pending === 'starting' ? 'Starting…' : this.pending === 'stopping' ? 'Stopping…' : this.lifecycle === 'running' ? 'Stop proxy' : 'Start proxy';
  }
  toggle(): void { if (this.ready && !this.pending) this.$emit('toggle-proxy'); }
}
ProxyToggle.define('proxy-toggle');

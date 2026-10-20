import {invoke} from '@tauri-apps/api/core';
import type {VisualizerHandle} from './models.js';

interface State {revision:number;handle:VisualizerHandle;busy:boolean;}
interface Host {state():State|null;replace():Promise<void>;notice(message:string):void;}
interface Status {available:boolean;enabled:boolean;dirty:boolean;requested:string|null;error:string|null;}

/** Only a visible development result supplies state. Installed/inactive viewers never poll. */
export class ReloadObserver {
  private timer:ReturnType<typeof setTimeout>|null=null;
  private polling=false;
  private generation=0;
  private enabled=false;
  private again=false;
  constructor(private readonly host:Host) {}
  stop():void {this.generation++;this.enabled=false;this.again=false;if(this.timer!==null)clearTimeout(this.timer);this.timer=null;}
  checkSoon():void {if(!this.host.state())return;this.again=true;if(!this.polling)this.schedule(0);}
  private schedule(delay=500):void {
    if(this.timer!==null)clearTimeout(this.timer);this.timer=null;
    if(!this.host.state()||!this.enabled&&!this.again)return;
    this.timer=setTimeout(()=>{this.timer=null;void this.check();},delay);
  }
  private async check():Promise<void> {
    const state=this.host.state();if(!state||this.polling)return;
    if(state.busy){this.again=true;this.schedule();return;}
    const generation=this.generation;this.polling=true;this.again=false;
    try {
      const status=await invoke<Status>('visualizer_reload_status',{handle:state.handle});
      if(generation!==this.generation||this.host.state()?.revision!==state.revision)return;
      this.enabled=status.available&&status.enabled&&!status.error;
      if(status.error)this.host.notice(status.error+' Use Settings → Extensions to reload.');
      if(status.available&&(status.dirty||status.requested)){
        await this.host.replace();
        // A settings request can arrive during replacement, including with hot reload off.
        if(generation===this.generation)this.again=true;
      }
    }catch {if(generation===this.generation&&this.host.state()?.revision===state.revision){this.enabled=false;this.host.notice('Source reload status is unavailable. Use Settings → Extensions to try again.');}}
    finally {this.polling=false;this.schedule();}
  }
}

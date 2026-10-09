import { WebUIElement } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import type { NoticeAction, ViewName, TracePasswordPrompt, OperationError } from './models.js';
import { describeError } from './utilities.js';

/** Typed events keep workspace components independent of their containing shell. */
export abstract class WorkspaceElement extends WebUIElement {
  protected showError(title:string,message:string):Promise<void> {
    return new Promise(resolve=>this.$emit('operation-error',{title,message,resolve} satisfies OperationError));
  }
  protected promptTracePassword(title:string,confirm=false,message=''):Promise<string|null> {
    return new Promise(resolve=>this.$emit('trace-password-request',{title,confirm,message,resolve} satisfies TracePasswordPrompt));
  }
  protected async withTracePassword<T>(title:string,operation:(password:string|null)=>Promise<T>):Promise<T|null> {
    let password:string|null=null;
    try {for(;;) {try {return await operation(password);}catch(error:unknown) {
      const category=typeof error==='object'&&error!==null&&'category' in error?String(error.category):'';
      const message=describeError(error);
      if(!['password-required','invalid-password'].includes(category)&&!/(requires a password|password is incorrect)/i.test(message))throw error;
      password=null;
      password=await this.promptTracePassword(title,false,category==='invalid-password'||/incorrect/i.test(message)?message:'');
      if(password===null)return null;
    }}} finally {password=null;}
  }
  protected set diagnostic(message: string) { this.$emit('diagnostic', message); }
  protected showNotice(title: string, message: string, actionLabel: string | null, action: NoticeAction): void {
    this.$emit('notice', { title, message, actionLabel, action });
  }
  protected activateView(view: ViewName): void { this.$emit('navigate', view); }
  protected async reportFrontendIssue(code: string, error: unknown): Promise<void> {
    await invoke<void>('record_frontend_diagnostic', { code, message: describeError(error) }).catch(() => undefined);
  }
}

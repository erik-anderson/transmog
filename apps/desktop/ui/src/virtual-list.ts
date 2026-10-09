/** Logical row geometry. DOM lifetime never determines identity or selection. */
export class VirtualList {
  ids:string[]=[];
  indexes=new Map<string,number>();
  private sizes=new Map<string,number>();
  private tree=new Float64Array(1);
  private estimate=32;

  reset(ids:string[],estimate=this.estimate,forgetSizes=false):void {
    this.ids=ids;this.estimate=estimate;this.indexes=new Map(ids.map((id,index)=>[id,index]));
    if(forgetSizes)this.sizes.clear();
    else for(const id of this.sizes.keys())if(!this.indexes.has(id))this.sizes.delete(id);
    this.tree=new Float64Array(ids.length+1);
    for(let index=1;index<=ids.length;index++){
      this.tree[index]!+=this.size(index-1);
      const parent=index+(index&-index);
      if(parent<=ids.length)this.tree[parent]!+=this.tree[index]!;
    }
  }
  size(index:number):number {return this.sizes.get(this.ids[index]??'')??this.estimate;}
  offset(index:number):number {
    let total=0;
    for(let cursor=Math.min(index,this.ids.length);cursor>0;cursor-=cursor&-cursor)total+=this.tree[cursor]!;
    return total;
  }
  get total():number {return this.offset(this.ids.length);}
  uniformSize(size:number):boolean {
    if(!Number.isFinite(size)||size<=0||Math.abs(size-this.estimate)<.001)return false;
    this.reset(this.ids,size,true);return true;
  }
  measure(id:string,size:number):boolean {
    const index=this.indexes.get(id);if(index===undefined||!Number.isFinite(size)||size<=0)return false;
    const delta=size-this.size(index);if(Math.abs(delta)<.25)return false;
    this.sizes.set(id,size);
    for(let cursor=index+1;cursor<this.tree.length;cursor+=cursor&-cursor)this.tree[cursor]!+=delta;
    return true;
  }
  indexAt(offset:number):number {
    let index=0,remaining=Math.max(0,offset),bit=1;
    while(bit*2<=this.ids.length)bit*=2;
    for(;bit;bit>>=1){const next=index+bit;if(next<this.tree.length&&this.tree[next]!<=remaining){remaining-=this.tree[next]!;index=next;}}
    return Math.min(index,Math.max(0,this.ids.length-1));
  }
  range(offset:number,height:number,overscan=10):{start:number;end:number} {
    if(!this.ids.length)return {start:0,end:0};
    return {start:Math.max(0,this.indexAt(offset)-overscan),end:Math.min(this.ids.length,this.indexAt(offset+height)+overscan+1)};
  }
  anchor(offset:number):{id:string;delta:number}|null {
    if(!this.ids.length)return null;const index=this.indexAt(offset);
    return {id:this.ids[index]!,delta:offset-this.offset(index)};
  }
  restore(anchor:{id:string;delta:number}|null):number|null {
    const index=anchor?this.indexes.get(anchor.id):undefined;
    return index===undefined?null:this.offset(index)+Math.min(anchor?.delta??0,Math.max(0,this.size(index)-.01));
  }
}

/* Proxy, Clash, node, subscription, core and diagnostics pages. Agent text is escaped;
 * flags were turned into letters when the records were read.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
static const char *page_title(void) {
  switch (page) {
  case WG: return "WireGuard";
  case PROFILES: return L("Saved profiles","已保存配置");
  case DEVICES: return L("Device routing","设备分流");
  case OVERVIEW: return L("Overview","概览");
  case PROXY: return L("Proxy","代理");
  case CLASH: return "Clash";
  case NODES: return L("Nodes","节点");
  case GROUP: return nodes.name[0]?nodes.name:L("Nodes","节点");
  case SUBS: return L("Subscriptions","订阅");
  case CORE: return L("Core & data","内核与数据");
  case DIAG: return L("Diagnostics","诊断");
  case ESIM: return "eSIM";
  default: return "OpenUI";
  }
}
static const char *reason_text(const char *code) {
  static const struct { const char *code, *en, *zh; } reasons[] = {
    {"kernel","Kernel support missing","内核不支持"},
    {"tools","WireGuard tools missing","缺少 WireGuard 工具"},
    {"config","Import a config in OpenUI","请在 OpenUI 导入配置"},
    {"invalid","Saved config is invalid","已保存的配置无效"},
    {"core","Clash core missing","缺少 Clash 内核"},
    {"tun","TUN is not supported","不支持 TUN"},
    {"sub","Add a subscription in OpenUI","请在 OpenUI 添加订阅"},
    {"download","Update the subscription first","请先更新订阅"},
  };
  for(size_t i=0;i<sizeof(reasons)/sizeof(reasons[0]);i++)
    if(!strcmp(code,reasons[i].code))return L(reasons[i].en,reasons[i].zh);
  return L("Not ready; see OpenUI","未就绪，请查看 OpenUI");
}
static const char *mode_text(const char *m) {
  return !strcmp(m,"global")?L("Global","全局"):!strcmp(m,"direct")?L("Direct","直连"):L("Rule","规则");
}
static const char *group_type(const char *t) {
  return !strcmp(t,"Selector")?L("Manual","手动选择"):!strcmp(t,"URLTest")?L("Auto (fastest)","自动测速"):
    !strcmp(t,"Fallback")?L("Fallback","故障转移"):!strcmp(t,"LoadBalance")?L("Load balance","负载均衡"):
    !strcmp(t,"Relay")?L("Relay","链式代理"):t;
}
static void delay_text(char *buf, size_t n, long long d) {
  if(d<0)snprintf(buf,n,"--");else if(!d)snprintf(buf,n,"%s",L("Timeout","超时"));else snprintf(buf,n,"%lld ms",d);
}
/* The web page's delay colours: under 200 ms good, under 500 ms fair. */
static const char *delay_class(long long d) { return d<0?"muted":!d?"bad":d<200?"good":d<500?"warn":"bad"; }
static void ago_text(char *buf, size_t n, unsigned long long t) {
  unsigned long long now=(unsigned long long)time(NULL),s=now>t?now-t:0;
  if(!t)snprintf(buf,n,"%s",L("never","从未"));
  else if(s<60)snprintf(buf,n,"%s",L("just now","刚刚"));
  else if(s<3600)snprintf(buf,n,L("%llu min ago","%llu 分钟前"),s/60);
  else if(s<86400)snprintf(buf,n,L("%llu h ago","%llu 小时前"),s/3600);
  else snprintf(buf,n,L("%llu d ago","%llu 天前"),s/86400);
}
/* "used / total · N days left" for a subscription; returns the class of the expiry. */
static const char *usage_text(char *buf, size_t n, const struct sub_info *s) {
  char used[24],total[24],left[48]="";
  const char *cls="muted";
  unsigned long long now=(unsigned long long)time(NULL);
  if(s->expire&&s->expire<=now){snprintf(left,sizeof(left),"%s",L("Expired","已过期"));cls="bad";}
  else if(s->expire){
    unsigned long long days=(s->expire-now)/86400;
    snprintf(left,sizeof(left),L("%llu days left","剩余 %llu 天"),days);
    if(days<7)cls="warn";
  }
  amount(used,sizeof(used),s->up+s->down);amount(total,sizeof(total),s->total);
  if(s->total)snprintf(buf,n,"%s / %s%s%s",used,total,left[0]?" · ":"",left);
  else if(s->up||s->down)snprintf(buf,n,L("%s used%s%s","已用 %s%s%s"),used,left[0]?" · ":"",left);
  else snprintf(buf,n,"%s",left[0]?left:L("No traffic information","无流量信息"));
  return cls;
}
static const struct sub_info *active_sub(void) {
  for(int i=0;i<data.nsubs;i++)if(data.subs[i].active)return &data.subs[i];
  return NULL;
}
static const char *clash_summary(void) {
  const struct clash_info *c=&data.clash;
  return !data.stamp?L("Loading status","正在加载状态"):stale()?L("Status is stale","状态已过期"):
    applying(1)?L("Applying changes","正在应用更改"):!c->has?L("Clash unavailable","Clash 不可用"):
    !c->enabled?L("Off","已关闭"):c->error[0]?L("Error","错误"):!c->running?L("Starting","正在启动"):
    c->connected?L("Connected","已连接"):L("Checking policy","正在检查策略");
}
static const char *clash_class(void) {
  const struct clash_info *c=&data.clash;
  return !data.stamp||applying(1)?"warn":stale()||(c->has&&c->enabled&&c->error[0])?"bad":
    !c->has||!c->enabled?"muted":c->running&&c->connected?"good":"warn";
}
static const char *proxy_summary(void) {
  const struct backend_info *b=&data.backend;
  if(b->has&&b->active==BK_WG)return summary();
  if(b->has&&b->active==BK_CLASH)return clash_summary();
  return !data.stamp||!b->has?L("Loading status","正在加载状态"):stale()?L("Status is stale","状态已过期"):
    applying(0)||applying(1)?L("Applying changes","正在应用更改"):L("Proxy off","代理已关闭");
}
static const char *proxy_class(void) {
  const struct backend_info *b=&data.backend;
  if(b->has&&b->active==BK_WG)return summary_class();
  if(b->has&&b->active==BK_CLASH)return clash_class();
  return !data.stamp||!b->has||applying(0)||applying(1)?"warn":stale()?"bad":"muted";
}
static const char *verdict_text(const char *v, const char **cls) {
  *cls="bad";
  if(!strcmp(v,"ok")){*cls="good";return L("All checks passed","全部检查通过");}
  if(!strcmp(v,"clash_off")){*cls="muted";return L("Clash is off","Clash 未开启");}
  if(!strcmp(v,"core_down"))return L("The Clash core does not answer","Clash 内核无响应");
  if(!strcmp(v,"not_routed"))return L("Device traffic does not reach Clash","设备流量未进入 Clash");
  if(!strcmp(v,"nodes"))return L("Nodes or the internet fail","节点或外网不可用");
  if(!strcmp(v,"no_devices")){*cls="warn";return L("No device uses Clash","没有设备走 Clash");}
  *cls="warn";return v;
}
static const char *diag_summary(const char **cls) {
  const struct diag_info *d=&data.diag;
  *cls="muted";
  if(!d->has)return data.stamp?L("Unavailable","不可用"):L("Loading status","正在加载状态");
  if(d->running){*cls="warn";return L("Running...","正在诊断...");}
  if(!d->verdict[0])return L("Find out why devices cannot connect","查找设备无法上网的原因");
  return verdict_text(d->verdict,cls);
}
/* A card or, with an id, a tappable row; children are placed relative to it. */
static void card_open(int x, int y, int w, int h, const char *id) {
  if(id)add("<a id=\"%s\" href=\"act:%s\" class=\"box row\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">",id,id,x,y,w,h);
  else add("<div class=\"box card\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">",x,y,w,h);
}
static void card_close(const char *id, int h) {
  if(id){chevron(h);add("</a>");}else add("</div>");
}
/* The footer line: stale status, a notice, or a hint. */
static void footer(int y, int h, const char *hint, const char *hint_cls) {
  const char *cls=h>20?"wrap ":"";
  char c[32];
  if(stale()){snprintf(c,sizeof(c),"%sbad",cls);box(16,y,292,h,c,L("Status is stale. Controls are disabled.","状态已过期，控制已停用。"));}
  else if(notice[0]){snprintf(c,sizeof(c),"%sbad",cls);box(16,y,292,h,c,notice);}
  else if(hint&&hint[0]){snprintf(c,sizeof(c),"%s%s",cls,hint_cls);box(16,y,292,h,c,hint);}
}
static void render_proxy(void) {
  const struct backend_info *b=&data.backend;
  char tmp[256],rx[32],tx[32];
  header(L("Proxy","代理"));
  box(16,72,292,20,"muted",L("How devices reach the internet","设备的上网方式"));
  const char *labels[3]={L("Off","关闭"),"WireGuard","Clash"};
  /* With both saved as on, every choice resolves the conflict, so none is shown as chosen. */
  for(int i=0;i<3;i++) {
    char id[8];snprintf(id,sizeof(id),"bk%d",i);
    int on=b->has&&b->active==i&&!b->conflict;
    linkbox(12+100*i,96,96,56,id,labels[i],b->has&&!blocked()&&!on&&(i==BK_OFF||b->ready[i-1]),on?CHOSEN:PLAIN);
  }
  int lines=0;
  for(int i=0;i<2&&b->has;i++) if(!b->ready[i]&&b->active!=i+1) {
    snprintf(tmp,sizeof(tmp),"%s: %s",i?"Clash":"WireGuard",reason_text(b->reason[i]));
    sbox(16,160+22*lines++,292,20,"warn",tmp);
  }
  if(!lines)box(16,160,292,20,"muted",L("Switching asks for confirmation.","切换前会请求确认。"));
  int target=b->active==BK_WG?WG:b->active==BK_CLASH?CLASH:-1;
  const char *link=b->has&&target>=0&&reachable(target)?"active":NULL;
  card_open(12,208,296,124,link);
  const char *cls=proxy_class();
  dot(16,15,tone_color(cls));
  snprintf(tmp,sizeof(tmp),"%s: %s",b->has&&b->active==BK_WG?"WireGuard":b->has&&b->active==BK_CLASH?"Clash":L("Proxy","代理"),proxy_summary());
  char c[24];snprintf(c,sizeof(c),"head %s",strcmp(cls,"muted")?cls:"");
  sbox(34,10,236,20,c,tmp);
  if(b->has&&b->active==BK_WG) {
    if(data.enabled&&data.handshake)snprintf(tmp,sizeof(tmp),L("Last handshake: %llu s ago","上次握手：%llu 秒前"),(unsigned long long)time(NULL)>data.handshake?(unsigned long long)time(NULL)-data.handshake:0);
    else snprintf(tmp,sizeof(tmp),"%s",L("Waiting for the server","正在等待服务器响应"));
    sbox(16,36,252,20,"muted",tmp);
    amount(rx,sizeof(rx),data.rx);amount(tx,sizeof(tx),data.tx);
    snprintf(tmp,sizeof(tmp),"↓ %s  ↑ %s",rx,tx);box(16,58,252,20,"muted",tmp);
    snprintf(tmp,sizeof(tmp),L("Profile: %s","当前配置：%s"),active_profile());sbox(16,80,252,20,"muted",tmp);
  } else if(b->has&&b->active==BK_CLASH) {
    const struct sub_info *s=active_sub();
    snprintf(tmp,sizeof(tmp),L("Mode: %s · %s","模式：%s · %s"),mode_text(data.clash.mode),s?s->name:L("no subscription","未选择订阅"));
    sbox(16,36,252,20,"muted",tmp);
    amount(rx,sizeof(rx),data.clash.memory_kib*1024);
    snprintf(tmp,sizeof(tmp),"%s · %s",data.clash.version[0]?data.clash.version:"Clash",rx);
    sbox(16,58,252,20,"muted",tmp);
    if(data.clash.error[0])sbox(16,80,252,20,"bad",data.clash.error);
  } else if(b->has) {
    box(16,36,252,20,"muted",L("Devices use the normal network.","设备使用普通网络。"));
    box(16,58,252,20,"muted",L("Choose WireGuard or Clash above.","可在上方选择 WireGuard 或 Clash。"));
  }
  if(b->has&&b->conflict)sbox(16,100,252,20,"warn",L("Both saved on: WireGuard is in effect.","两者均为开启：以 WireGuard 为准。"));
  card_close(link,124);
  const char *dcls;const char *detail=diag_summary(&dcls);
  nav_row(12,344,296,60,"diag",L("Diagnostics","诊断"),detail,dcls,reachable(DIAG));
  footer(414,40,pending&&pending_kind==JOB_BACKEND?L("Switching can take up to two minutes.","切换最多需要两分钟。"):
         L("Each backend keeps its own device list.","各后端分别保存设备列表。"),pending?"warn":"muted");
}
static void render_clash(void) {
  const struct clash_info *c=&data.clash;
  const struct backend_info *b=&data.backend;
  char tmp[256],mem[32];
  header("Clash");
  /* Status card, switch, modes, then four 48 px rows: nodes, subscriptions, core and
   * diagnostics (also on the Proxy page, which the menu may hide). */
  add("<div class=\"box card\" style=\"left:12px;top:72px;width:296px;height:68px\"></div>");
  const char *cls=clash_class();
  dot(28,81,tone_color(cls));
  snprintf(tmp,sizeof(tmp),"head %s",strcmp(cls,"muted")?cls:"");
  sbox(46,76,250,20,tmp,clash_summary());
  const struct sub_info *s=active_sub();
  if(c->has&&!c->installed)snprintf(tmp,sizeof(tmp),"%s",L("Clash core missing","缺少 Clash 内核"));
  else if(s)snprintf(tmp,sizeof(tmp),L("Subscription: %s","订阅：%s"),s->name);
  else snprintf(tmp,sizeof(tmp),"%s",L("No subscription selected","未选择订阅"));
  sbox(28,96,268,20,"muted",tmp);
  const char *line_cls="muted";
  if(c->error[0]){snprintf(tmp,sizeof(tmp),"%s",c->error);line_cls="bad";}
  else if(c->running){amount(mem,sizeof(mem),c->memory_kib*1024);snprintf(tmp,sizeof(tmp),"%s · %s",c->version[0]?c->version:"Clash",mem);}
  else if(s)line_cls=usage_text(tmp,sizeof(tmp),s);
  else tmp[0]=0;
  sbox(28,116,268,20,line_cls,tmp);
  int can=b->has&&c->has&&!blocked()&&(c->enabled?b->active==BK_CLASH:b->ready[1]);
  const char *label=pending&&pending_kind==JOB_BACKEND?L("Switching...","正在切换..."):c->enabled?L("Turn off Clash","关闭 Clash"):
    b->active==BK_WG?L("Switch to Clash","切换到 Clash"):L("Turn on Clash","开启 Clash");
  linkbox(12,144,296,48,"ctoggle",label,can,c->enabled?DANGER:PRIMARY);
  static const char *modes[]={"rule","global","direct"};
  for(int i=0;i<3;i++) {
    char id[8];snprintf(id,sizeof(id),"m%d",i);
    int on=c->has&&!strcmp(c->mode,modes[i]);
    linkbox(12+100*i,196,96,48,id,mode_text(modes[i]),c->has&&!blocked()&&!on,on?CHOSEN:PLAIN);
  }
  nav_row(12,248,296,48,"nodes",L("Nodes","节点"),c->running?L("Groups, nodes and delay tests","分组、节点与测速"):
          L("Available while Clash runs","Clash 运行时可用"),"",c->running&&reachable(NODES));
  if(data.nsubs)snprintf(tmp,sizeof(tmp),L("%d saved · switch or update","已保存 %d 个 · 切换或更新"),data.nsubs);
  else snprintf(tmp,sizeof(tmp),"%s",L("Add subscriptions in OpenUI","请在 OpenUI 添加订阅"));
  nav_row(12,300,296,48,"subs",L("Subscriptions","订阅"),tmp,"",reachable(SUBS));
  const struct core_info *k=&data.core;
  if(k->available&&k->newer&&k->latest[0])snprintf(tmp,sizeof(tmp),L("%s · %s available","%s · 可更新到 %s"),k->version[0]?k->version:"--",k->latest);
  else snprintf(tmp,sizeof(tmp),"%s",k->available&&k->version[0]?k->version:L("Core and geo data","内核与 GEO 数据"));
  nav_row(12,352,296,48,"core",L("Core & data","内核与数据"),tmp,k->newer?"accent":"",reachable(CORE));
  const char *dcls;const char *detail=diag_summary(&dcls);
  nav_row(12,404,296,48,"diag",L("Diagnostics","诊断"),detail,dcls,reachable(DIAG));
  footer(456,20,b->has&&!b->ready[1]&&!c->enabled?reason_text(b->reason[1]):"",
         b->has&&!b->ready[1]&&!c->enabled?"warn":"muted");
}
static const char *mode_hint(void) {
  const char *m=data.clash.mode;
  return !strcmp(m,"global")?L("Global mode: only GLOBAL decides.","全局模式：仅 GLOBAL 分组生效。"):
    !strcmp(m,"direct")?L("Direct mode: node choices are kept but unused.","直连模式：节点选择保留但不生效。"):
    L("Rule mode: sites follow the profile's rules.","规则模式：按配置规则分流。");
}
static void render_nodes(void) {
  char tmp[512];
  header(L("Nodes","节点"));
  const char *cls="muted";
  if(groups.state==1)snprintf(tmp,sizeof(tmp),L("%d groups · mode: %s","%d 个分组 · 模式：%s"),groups.total,mode_text(data.clash.mode));
  else if(groups.state==0)snprintf(tmp,sizeof(tmp),"%s",L("Loading groups...","正在加载分组..."));
  else {cls="bad";snprintf(tmp,sizeof(tmp),"%s",groups.state==2?L("Clash is not running.","Clash 未运行。"):L("The node list is unavailable.","无法读取节点列表。"));}
  box(16,72,292,20,cls,tmp);
  for(int i=0;groups.state==1&&i<groups.n;i++) {
    const struct group_row *r=&groups.rows[i];
    char id[8];snprintf(id,sizeof(id),"g%d",i);
    snprintf(tmp,sizeof(tmp),"%s · %s",group_type(r->type),r->now_name[0]?r->now_name:"--");
    nav_row(12,100+62*i,296,56,id,r->name[0]?r->name:"?",tmp,"",1);
  }
  if(groups.state==1&&!groups.total)box(28,180,264,40,"wrap muted center",L("No proxy groups","没有代理分组"));
  linkbox(12,356,100,48,"prev",L("‹ Prev","‹ 上页"),groups.state==1&&group_offset>0,PLAIN);
  linkbox(208,356,100,48,"next",L("Next ›","下页 ›"),groups.state==1&&group_offset+4<groups.total,PLAIN);
  snprintf(tmp,sizeof(tmp),"%d / %d",group_offset/4+1,groups.total?(groups.total+3)/4:1);box(112,370,96,20,"muted center",tmp);
  footer(416,40,mode_hint(),"muted");
}
static void render_group(void) {
  char tmp[512],d[24];
  header(page_title());
  if(!nodes.ok)box(16,72,292,20,"muted",L("Loading nodes...","正在加载节点..."));
  else {
    snprintf(tmp,sizeof(tmp),nodes.selectable?L("%s · %d nodes · tap to use","%s · %d 个节点，点击使用"):
             L("%s · %d nodes · automatic","%s · %d 个节点，自动选择"),group_type(nodes.type),nodes.total);
    sbox(16,72,292,20,"muted",tmp);
  }
  for(int i=0;nodes.ok&&i<nodes.n;i++) {
    const struct node_row *r=&nodes.rows[i];
    int current=r->index==nodes.now, tappable=nodes.selectable&&!current&&!blocked();
    char id[8];snprintf(id,sizeof(id),"n%d",i);
    add("<a id=\"%s\" href=\"act:%s\" class=\"box row%s\" style=\"left:12px;top:%dpx;width:296px;height:56px\">",id,tappable?id:"none",current?" current":"",100+62*i);
    snprintf(tmp,sizeof(tmp),"%s%s",current?"✓ ":"",r->name[0]?r->name:"?");
    add("<div class=\"name\" style=\"right:96px\">");shortened(tmp,186,16);
    add("</div><div class=\"sub\" style=\"right:96px\">");escaped(r->type);add("</div>");
    delay_text(d,sizeof(d),r->delay);
    add("<div class=\"box head right %s\" style=\"left:196px;top:17px;width:86px;height:20px\">",delay_class(r->delay));escaped(d);add("</div></a>");
  }
  linkbox(12,356,92,48,"prev",L("‹ Prev","‹ 上页"),nodes.ok&&node_offset>0,PLAIN);
  linkbox(112,356,96,48,"test",pending&&pending_kind==JOB_DELAY?L("Testing","测速中"):L("Test","测速"),nodes.ok&&!blocked(),PRIMARY);
  linkbox(216,356,92,48,"next",L("Next ›","下页 ›"),nodes.ok&&node_offset+4<nodes.total,PLAIN);
  snprintf(tmp,sizeof(tmp),"%d / %d",node_offset/4+1,nodes.total?(nodes.total+3)/4:1);box(112,410,96,20,"muted center",tmp);
  const char *hint=pending&&pending_kind==JOB_DELAY?L("Testing every node, up to 20 s.","正在测试全部节点，最多 20 秒。"):
    pending&&pending_kind==JOB_SELECT?L("Switching node...","正在切换节点..."):
    nodes.ok&&!nodes.selectable?L("This group picks its node itself.","此分组自动选择节点。"):"";
  footer(436,40,hint,pending?"warn":"muted");
}
static void render_subs(void) {
  char tmp[256];
  header(L("Subscriptions","订阅"));
  snprintf(tmp,sizeof(tmp),L("%d / 5 saved · tap for actions","已保存 %d / 5 个，点击操作"),data.nsubs);
  box(16,72,292,20,"muted",tmp);
  for(int i=0;i<data.nsubs;i++) {
    const struct sub_info *s=&data.subs[i];
    char id[8];snprintf(id,sizeof(id),"s%d",i);
    open_row(12,96+64*i,296,58,id,!stale(),s->active?"current":"");
    add("<div class=\"box head\" style=\"left:14px;top:8px;width:%dpx;height:20px\">",s->active?200:236);shortened(s->name[0]?s->name:"?",s->active?200:236,16);add("</div>");
    if(s->active)add("<div class=\"box badge\" style=\"right:12px;top:7px;width:56px;height:22px\">%s</div>",L("Active","当前"));
    else chevron(58);
    const char *cls="muted";
    if(!s->downloaded){snprintf(tmp,sizeof(tmp),"%s",L("Not downloaded yet","尚未下载"));cls="warn";}
    else if(s->failed){snprintf(tmp,sizeof(tmp),"%s",L("Last update failed","上次更新失败"));cls="bad";}
    else cls=usage_text(tmp,sizeof(tmp),s);
    add("<div class=\"box %s\" style=\"left:14px;top:31px;width:236px;height:20px\">",stale()?"":cls);shortened(tmp,236,16);add("</div></a>");
  }
  if(!data.nsubs)box(28,150,264,60,"wrap muted",L("Add subscriptions in the OpenUI webpage first.","请先在 OpenUI 网页中添加订阅。"));
  footer(420,40,pending&&pending_kind==JOB_REFRESH?L("Updating the subscription...","正在更新订阅..."):
         L("Adding and editing stay in OpenUI.","添加与编辑请在 OpenUI 中进行。"),pending?"warn":"muted");
}
static void render_sub_dialog(void) {
  const struct sub_info *s=NULL;
  char tmp[256],ago[48];
  for(int i=0;i<data.nsubs;i++)if(data.subs[i].id==chosen_sub)s=&data.subs[i];
  dialog_card(80,368);
  if(!s) {
    box(28,96,264,32,"title",L("Subscription removed","订阅已删除"));
    linkbox(24,356,272,52,"dismiss",L("Close","关闭"),1,PLAIN);
    return;
  }
  add("<div class=\"box title\" style=\"left:28px;top:94px;width:264px;height:32px\">");shortened(s->name[0]?s->name:"?",264,24);add("</div>");
  const char *cls=usage_text(tmp,sizeof(tmp),s);
  sbox(28,134,264,20,cls,tmp);
  ago_text(ago,sizeof(ago),s->updated);
  snprintf(tmp,sizeof(tmp),L("Updated %s · %d nodes","%s更新 · %d 个节点"),ago,s->proxies);
  sbox(28,156,264,20,"muted",tmp);
  if(!s->downloaded)box(28,178,264,20,"warn",L("Not downloaded yet","尚未下载"));
  else if(s->failed)box(28,178,264,20,"bad",L("Last update failed","上次更新失败"));
  else if(s->active)box(28,178,264,20,"good",L("In use","正在使用"));
  linkbox(24,212,272,52,"activate",s->active?L("Current subscription","当前订阅"):L("Use this subscription","设为当前"),!blocked()&&!s->active,PRIMARY);
  linkbox(24,272,272,52,"srefresh",pending&&pending_kind==JOB_REFRESH?L("Updating...","正在更新..."):L("Update now","立即更新"),!blocked(),PLAIN);
  linkbox(24,332,272,52,"dismiss",L("Cancel","取消"),1,PLAIN);
  if(data.clash.enabled)box(28,394,264,40,"wrap muted",L("Switching reloads Clash; traffic pauses briefly.","切换时 Clash 会重新加载，流量短暂中断。"));
}
static const char *source_text(const char *s) {
  return !strcmp(s,"pinned")?L("Bundled","内置"):!strcmp(s,"official")?L("Official release","官方发布"):
    !strcmp(s,"uploaded")?L("Uploaded","手动上传"):s[0]?L("Unknown","未知"):"--";
}
static const char *job_kind_text(const char *k) {
  return !strcmp(k,"check")?L("Update check","检查更新"):!strcmp(k,"update")?L("Core update","内核更新"):
    !strcmp(k,"rollback")?L("Rollback","回滚"):!strcmp(k,"geodata")?L("Geo data","GEO 数据"):
    !strcmp(k,"upload")?L("Upload","上传"):k;
}
static const char *job_step_text(const char *s) {
  static const struct { const char *step, *en, *zh; } steps[] = {
    {"checking","checking","检查中"},{"downloading","downloading","下载中"},{"extracting","extracting","解压中"},
    {"verifying","verifying","校验中"},{"testing","test-running","试运行中"},{"installing","installing","安装中"},
    {"restarting","restarting","重启中"},{"receiving","receiving","接收中"},
  };
  for(size_t i=0;i<sizeof(steps)/sizeof(steps[0]);i++)if(!strcmp(s,steps[i].step))return L(steps[i].en,steps[i].zh);
  return s;
}
/* A release tag goes back to the agent only if it is a plain version word. */
static int tag_ok(const char *t) {
  size_t n=strlen(t);
  if(!n||n>40)return 0;
  for(;*t;t++)if(!((*t>='0'&&*t<='9')||(*t>='a'&&*t<='z')||(*t>='A'&&*t<='Z')||strchr("._-",*t)))return 0;
  return 1;
}
/* Core actions need the updater, no other job and no running core job; "Update core"
 * also a newer release whose tag can go back. The page and its taps share these. */
static int core_idle(void) {
  const struct core_info *c=&data.core;
  return c->available&&!blocked()&&strcmp(c->state,"running");
}
static int core_update_ok(void) { return core_idle()&&data.core.newer&&tag_ok(data.core.latest); }
static void kv_row(int y, const char *label, const char *value, const char *cls) {
  box(28,y,118,20,"muted",label);
  char c[32];snprintf(c,sizeof(c),"head right %s",cls);
  sbox(146,y,148,20,c,value);
}
static void render_core(void) {
  const struct core_info *c=&data.core;
  char tmp[256],a[32],b[32];
  header(L("Core & data","内核与数据"));
  add("<div class=\"box card\" style=\"left:12px;top:72px;width:296px;height:120px\"></div>");
  kv_row(80,L("Installed","已安装"),c->version[0]?c->version:"--","");
  kv_row(102,L("Source","来源"),source_text(c->source),"");
  if(c->latest[0])snprintf(tmp,sizeof(tmp),"%s%s",c->latest,c->newer?L(" (new)","（新）"):" ✓");
  else snprintf(tmp,sizeof(tmp),"%s",L("Not checked","未检查"));
  kv_row(124,L("Latest","最新"),tmp,c->newer?"accent":"");
  kv_row(146,L("Previous","上一版本"),c->previous[0]?c->previous:L("None","无"),"");
  amount(a,sizeof(a),c->free);
  kv_row(168,L("Free space","可用空间"),c->available?a:"--","");
  int running=!strcmp(c->state,"running");
  const char *cls="muted";
  if(!c->has)snprintf(tmp,sizeof(tmp),"%s",L("Loading status","正在加载状态"));
  else if(!c->available){snprintf(tmp,sizeof(tmp),"%s",L("Core updates are not available.","不支持内核更新。"));cls="warn";}
  else if(!c->kind[0])snprintf(tmp,sizeof(tmp),"%s",L("No core job yet.","尚无内核任务。"));
  else {
    snprintf(tmp,sizeof(tmp),"%s: %s",job_kind_text(c->kind),running?job_step_text(c->step):
             !strcmp(c->state,"done")?L("done","完成"):L("failed","失败"));
    cls=running?"warn":!strcmp(c->state,"done")?"good":"bad";
  }
  sbox(16,200,292,20,cls,tmp);
  tmp[0]=0;cls="muted";
  if(running&&c->total){amount(a,sizeof(a),c->bytes);amount(b,sizeof(b),c->total);snprintf(tmp,sizeof(tmp),"%s / %s",a,b);}
  else if(!running&&c->error[0]){snprintf(tmp,sizeof(tmp),"%s",c->error);cls="bad";}
  else if(c->latest[0]&&c->published[0])snprintf(tmp,sizeof(tmp),L("Latest release: %s","最新发布：%s"),c->published);
  sbox(16,222,292,20,cls,tmp);
  int idle=core_idle();
  linkbox(12,256,142,56,"ccheck",running&&!strcmp(c->kind,"check")?L("Checking","检查中"):L("Check update","检查更新"),idle,PLAIN);
  linkbox(166,256,142,56,"cupdate",L("Update core","更新内核"),core_update_ok(),PRIMARY);
  linkbox(12,320,142,56,"crollback",L("Roll back","回滚"),idle&&c->previous[0],DANGER);
  linkbox(166,320,142,56,"cgeo",L("Update GEO","更新 GEO"),idle,PLAIN);
  footer(392,60,L("New files are verified and test-run first; a running core restarts.","新文件会先校验并试运行；运行中的内核会重启。"),"muted");
}
static const char *check_name(const char *id) {
  static const struct { const char *id, *en, *zh; } names[] = {
    {"clock","Clock","时钟"},{"core","Core","内核"},{"dns","DNS","DNS"},{"nodes","Nodes","节点"},
    {"fetch","Web access","外网访问"},{"devices","Devices","设备"},{"route","Routing","路由"},{"firewall","Firewall","防火墙"},
  };
  for(size_t i=0;i<sizeof(names)/sizeof(names[0]);i++)if(!strcmp(id,names[i].id))return L(names[i].en,names[i].zh);
  return id;
}
static const char *check_status(const char *s, const char **cls) {
  if(!strcmp(s,"pass")){*cls="good";return L("✓ Pass","✓ 通过");}
  if(!strcmp(s,"warn")){*cls="warn";return L("! Warning","! 注意");}
  if(!strcmp(s,"fail")){*cls="bad";return L("✗ Fail","✗ 失败");}
  *cls="muted";return L("- Skipped","- 跳过");
}
static const char *yes_no(long long v) { return v==NO_FIGURE?"?":v?"✓":"✗"; }
/* The check's figures in words; the agent sends numbers, not English sentences. */
static void check_detail(char *buf, size_t n, const struct check_info *c) {
  const long long *v=c->v;
  const char *id=c->id;
  buf[0]=0;
  if(!strcmp(c->status,"skip")){snprintf(buf,n,"%s",L("Not run","未运行"));return;}
  if(!strcmp(id,"clock")) {
    if(v[0]==NO_FIGURE)snprintf(buf,n,"%s",L("No time source reached","无法获取网络时间"));
    else snprintf(buf,n,L("Off by %lld s","偏差 %lld 秒"),v[0]<0?-v[0]:v[0]);
  } else if(!strcmp(id,"core"))snprintf(buf,n,"%s",v[0]==1?L("Controller answers","控制接口正常"):L("Core does not answer","内核无响应"));
  else if(!strcmp(id,"dns")||!strcmp(id,"fetch")) {
    if(!strcmp(id,"dns"))snprintf(buf,n,L("China %s · abroad %s","国内 %s · 国外 %s"),yes_no(v[0]),yes_no(v[1]));
    else {
      /* HTTP status codes; none means the request failed. */
      char code[2][24];
      for(int i=0;i<2;i++){if(v[i]==NO_FIGURE)snprintf(code[i],sizeof(code[i]),"✗");else snprintf(code[i],sizeof(code[i]),"%lld",v[i]);}
      snprintf(buf,n,L("China %s · abroad %s","国内 %s · 国外 %s"),code[0],code[1]);
    }
  } else if(!strcmp(id,"nodes")) {
    if(v[0]!=NO_FIGURE)snprintf(buf,n,L("HTTPS %lld ms","HTTPS %lld 毫秒"),v[0]);
    else if(v[1]!=NO_FIGURE)snprintf(buf,n,L("Only HTTP: %lld ms","仅 HTTP：%lld 毫秒"),v[1]);
    else snprintf(buf,n,"%s",L("The chosen node fails","所选节点不可用"));
    if(v[2]!=NO_FIGURE&&v[3]!=NO_FIGURE){size_t k=strlen(buf);snprintf(buf+k,n-k,L(" · %lld/%lld answer"," · %lld/%lld 可用"),v[2],v[3]);}
  } else if(!strcmp(id,"devices"))snprintf(buf,n,L("%lld of %lld devices use Clash","%lld / %lld 台设备走 Clash"),v[0]==NO_FIGURE?0:v[0],v[1]==NO_FIGURE?0:v[1]);
  else if(!strcmp(id,"route"))snprintf(buf,n,"%s",v[0]==1&&v[1]==1?L("Traffic and DNS enter the TUN","流量与 DNS 进入 TUN"):L("Not routed into the TUN","未进入 TUN"));
  else if(!strcmp(id,"firewall")) {
    if(v[2]==1)snprintf(buf,n,"%s",L("Transition guard still held","切换保护仍未解除"));
    else if(v[0]>0&&v[0]!=NO_FIGURE)snprintf(buf,n,L("Hooks missing: %lld","缺少挂载点：%lld 个"),v[0]);
    else if(v[1]>0&&v[1]!=NO_FIGURE)snprintf(buf,n,L("Hooks not first: %lld","挂载点未排在最前：%lld 个"),v[1]);
    else if(v[3]>0&&v[3]!=NO_FIGURE)snprintf(buf,n,L("Rejected packets: %lld","已拒绝数据包：%lld 个"),v[3]);
    else snprintf(buf,n,"%s",L("Rules in place","规则完整"));
  }
}
static void render_diag(void) {
  const struct diag_info *d=&data.diag;
  char tmp[256],ago[48];
  header(L("Diagnostics","诊断"));
  linkbox(12,72,296,56,"drun",d->running||(pending&&pending_kind==JOB_DIAG)?L("Running, up to 30 s","正在诊断，最多 30 秒"):L("Run diagnostics","运行诊断"),
          d->has&&!d->running&&!blocked(),PRIMARY);
  const char *cls,*v=diag_summary(&cls);
  snprintf(tmp,sizeof(tmp),"head %s",strcmp(cls,"muted")?cls:"");
  sbox(16,136,292,20,tmp,v);
  if(notice[0]||stale())footer(158,20,"","muted");
  else if(d->finished&&!d->running){ago_text(ago,sizeof(ago),d->finished);snprintf(tmp,sizeof(tmp),L("Finished %s","%s完成"),ago);box(16,158,292,20,"muted",tmp);}
  for(int i=0;i<4&&diag_offset+i<d->n;i++) {
    const struct check_info *c=&d->checks[diag_offset+i];
    int y=184+54*i;
    add("<div class=\"box card\" style=\"left:12px;top:%dpx;width:296px;height:50px\">",y);
    box(14,5,150,20,"head",check_name(c->id));
    const char *scls,*st=check_status(c->status,&scls);
    snprintf(tmp,sizeof(tmp),"head right %s",scls);box(166,5,116,20,tmp,st);
    check_detail(tmp,sizeof(tmp),c);sbox(14,26,268,20,"muted",tmp);
    add("</div>");
  }
  if(d->has&&!d->n&&!d->running)box(28,240,264,60,"wrap muted center",L("Checks clock, core, DNS, nodes, web access, devices, routing and firewall.","检查时钟、内核、DNS、节点、外网访问、设备、路由与防火墙。"));
  linkbox(12,404,100,48,"prev",L("‹ Prev","‹ 上页"),diag_offset>0,PLAIN);
  linkbox(208,404,100,48,"next",L("Next ›","下页 ›"),diag_offset+4<d->n,PLAIN);
  snprintf(tmp,sizeof(tmp),"%d / %d",diag_offset/4+1,d->n?(d->n+3)/4:1);box(112,418,96,20,"muted center",tmp);
}
/* eSIM actions need lpac on the device and no eSIM job running. */
static int esim_idle(void) {
  const struct esim_info *e=&data.esim;
  return e->has&&e->installed&&!blocked()&&strcmp(e->state,"running");
}
static const char *esim_kind_text(const char *k) {
  return !strcmp(k,"scan")?L("Reading","读卡"):!strcmp(k,"enable")?L("Switching","切换"):!strcmp(k,"disable")?L("Disabling","停用"):
    !strcmp(k,"nickname")?L("Renaming","改名"):!strcmp(k,"delete")?L("Deleting","删除"):!strcmp(k,"download")?L("Downloading","下载"):
    !strcmp(k,"notifications")?L("Notifications","通知"):k;
}
static const char *esim_step_text(const char *s) {
  static const struct { const char *step, *en, *zh; } steps[] = {
    {"reading","reading the card","读取卡片"},{"es10c_enable_profile","switching on the card","卡上切换"},
    {"es10c_disable_profile","disabling on the card","卡上停用"},{"radio_offline","radio off","关闭无线电"},
    {"sim_power_cycle","restarting the SIM","重启 SIM 卡"},{"radio_online","radio on","打开无线电"},
    {"sim_reading","modem reads the card","基带读卡"},{"firmware","waiting for the firmware","等待固件"},
    {"es10c_delete_profile","deleting","删除中"},{"es10c_set_nickname","saving","保存中"},
    {"es9p_handle_notification","sending notifications","发送通知"},
  };
  for(size_t i=0;i<sizeof(steps)/sizeof(steps[0]);i++)if(!strcmp(s,steps[i].step))return L(steps[i].en,steps[i].zh);
  return !strncmp(s,"es9p_",5)||!strncmp(s,"es10b_",6)||!strncmp(s,"es8p_",5)?L("downloading","下载中"):s;
}
static const char *esim_card_text(const struct esim_info *e, const char **cls) {
  *cls="";
  if(!e->has)return data.stamp?L("Unavailable","不可用"):L("Loading status","正在加载状态");
  if(!e->installed){*cls="warn";return L("lpac is not installed","设备上没有安装 lpac");}
  if(!strcmp(e->card,"euicc"))return L("eUICC card","eUICC 卡");
  if(!strcmp(e->card,"sim")){*cls="warn";return L("An ordinary SIM card","普通 SIM 卡");}
  if(!strcmp(e->card,"absent")){*cls="warn";return L("No card in the tray","卡槽中没有卡");}
  if(!strcmp(e->card,"error")){*cls="bad";return L("The card could not be read","无法读取卡片");}
  return L("Not read yet","尚未读取");
}
/* The agent's message, in a few words for the known ones; others as sent (English). */
static const char *esim_error_text(const char *e) {
  static const struct { const char *needle, *en, *zh; } known[] = {
    {"catBusy","Card busy: restart or reinsert it","卡片忙：请重启设备或重新插卡"},
    {"did not answer in time","The modem did not answer","基带没有响应"},
    {"SIM service is not reachable","The modem's SIM service is unavailable","无法连接基带的 SIM 服务"},
    {"still reads the previous profile","The modem still reads the old profile","基带仍读到原来的配置文件"},
    {"radio did not","The radio did not switch; restart","无线电未能切换，请重启设备"},
    {"not an eUICC","Not an eUICC card","不是 eUICC 卡"},
  };
  for(size_t i=0;i<sizeof(known)/sizeof(known[0]);i++)if(strstr(e,known[i].needle))return L(known[i].en,known[i].zh);
  return e;
}
/* Menu detail: the enabled profile, or what is in the tray. */
static const char *esim_summary(void) {
  const struct esim_info *e=&data.esim;
  if(e->has&&!strcmp(e->card,"euicc")) {
    for(int i=0;i<e->n;i++)if(e->rows[i].enabled)return e->rows[i].name[0]?e->rows[i].name:L("Enabled profile","已启用配置");
    return L("No profile enabled","没有启用的配置文件");
  }
  const char *cls;
  return e->has&&e->card[0]?esim_card_text(e,&cls):L("Profiles on a removable eUICC","可插拔 eUICC 配置文件");
}
static void render_esim(void) {
  const struct esim_info *e=&data.esim;
  char tmp[256];
  header("eSIM");
  add("<div class=\"box card\" style=\"left:12px;top:72px;width:296px;height:68px\"></div>");
  const char *cls,*card=esim_card_text(e,&cls);
  if(!strcmp(e->card,"euicc"))snprintf(tmp,sizeof(tmp),L("%s · %d profiles","%s · %d 个配置文件"),card,e->total);
  else snprintf(tmp,sizeof(tmp),"%s",card);
  char c[24];snprintf(c,sizeof(c),"head %s",cls);
  sbox(28,78,268,20,c,tmp);
  /* The running or last job, or what to do. */
  int running=!strcmp(e->state,"running");
  const char *line_cls="muted";
  if(running){snprintf(tmp,sizeof(tmp),"%s: %s",esim_kind_text(e->kind),esim_step_text(e->step));line_cls="warn";}
  else if(e->reboot){snprintf(tmp,sizeof(tmp),"%s",L("Restart the device to finish switching.","请重启设备以完成切换。"));line_cls="warn";}
  else if(!strcmp(e->state,"failed")&&e->error[0]){snprintf(tmp,sizeof(tmp),"%s",esim_error_text(e->error));line_cls="bad";}
  else if(!strcmp(e->card,"error")&&e->error[0]){snprintf(tmp,sizeof(tmp),"%s",esim_error_text(e->error));line_cls="bad";}
  else if(!strcmp(e->state,"done")&&!strcmp(e->kind,"enable")&&e->firmware_ready==1){snprintf(tmp,sizeof(tmp),"%s",L("Switched. Mobile data reconnects shortly.","已切换，移动数据稍后恢复。"));line_cls="good";}
  else if(!strcmp(e->card,"euicc"))snprintf(tmp,sizeof(tmp),"%s",e->n?L("Tap a profile to switch to it.","点击配置文件即可切换。"):L("Add profiles in OpenUI.","请在 OpenUI 中添加配置文件。"));
  else if(!strcmp(e->card,"sim"))snprintf(tmp,sizeof(tmp),"%s",L("Profiles need an eUICC card.","配置文件需要 eUICC 卡。"));
  else tmp[0]=0;
  sbox(28,104,268,20,line_cls,tmp);
  for(int i=0;i<4&&esim_offset+i<e->n;i++) {
    const struct esim_row *r=&e->rows[esim_offset+i];
    char id[8];snprintf(id,sizeof(id),"e%d",i);
    int on=r->enabled;
    open_row(12,148+56*i,296,52,id,!on&&esim_idle(),on?"current":"");
    add("<div class=\"box head\" style=\"left:14px;top:5px;width:%dpx;height:20px\">",on?200:236);shortened(r->name[0]?r->name:"?",on?200:236,16);add("</div>");
    if(on)add("<div class=\"box badge\" style=\"right:12px;top:14px;width:56px;height:22px\">%s</div>",L("Active","当前"));
    else chevron(52);
    snprintf(tmp,sizeof(tmp),"ICCID …%s%s%s",r->tail,!strcmp(r->cls,"test")?L(" · test"," · 测试"):"",!strcmp(r->cls,"provisioning")?L(" · provisioning"," · 预置"):"");
    add("<div class=\"box muted\" style=\"left:14px;top:27px;width:236px;height:20px\">");shortened(tmp,236,16);add("</div></a>");
  }
  if(e->n>4) {
    linkbox(12,370,100,48,"prev",L("‹ Prev","‹ 上页"),esim_offset>0,PLAIN);
    linkbox(208,370,100,48,"next",L("Next ›","下页 ›"),esim_offset+4<e->n,PLAIN);
    snprintf(tmp,sizeof(tmp),"%d / %d",esim_offset/4+1,(e->n+3)/4);box(112,384,96,20,"muted center",tmp);
  } else footer(384,20,L("Download, rename and delete in OpenUI.","下载、改名和删除请在 OpenUI 中进行。"),"muted");
  linkbox(12,422,142,50,"escan",running&&!strcmp(e->kind,"scan")?L("Reading","读取中"):L("Read again","重新读取"),esim_idle(),PLAIN);
  linkbox(166,422,142,50,"ereboot",L("Restart device","重启设备"),e->reboot&&!blocked(),DANGER);
}
static void render_confirm(void) {
  const struct backend_info *b=&data.backend;
  const char *title,*text,*button=L("Switch","切换");
  char detail[200]="";
  int tone=PRIMARY;
  if(confirm_what==CF_BACKEND&&confirm_target==BK_OFF) {
    title=L("Turn off the proxy?","关闭代理？");
    text=L("Devices will use the normal network.","设备将恢复为普通网络。");
    button=L("Turn off","关闭");tone=DANGER;
  } else if(confirm_what==CF_BACKEND&&confirm_target==BK_WG) {
    title=L("Switch to WireGuard?","切换到 WireGuard？");
    text=b->active==BK_CLASH?L("Clash turns off and traffic pauses briefly.","Clash 将关闭，流量会短暂中断。"):
      L("Selected devices will use WireGuard.","所选设备将通过 WireGuard 上网。");
  } else if(confirm_what==CF_BACKEND) {
    title=L("Switch to Clash?","切换到 Clash？");
    text=b->active==BK_WG?L("WireGuard turns off. Starting Clash can take a minute.","WireGuard 将关闭，启动 Clash 可能需要一分钟。"):
      L("Selected devices will use Clash. Starting can take a minute.","所选设备将通过 Clash 上网，启动可能需要一分钟。");
  } else if(confirm_what==CF_UPDATE) {
    title=L("Update the core?","更新内核？");
    text=L("It is downloaded, verified and test-run first. A running core restarts.","会先下载、校验并试运行；运行中的内核会重启。");
    snprintf(detail,sizeof(detail),"%s → %s",data.core.version[0]?data.core.version:"--",data.core.latest);
    button=L("Update","更新");
  } else if(confirm_what==CF_ROLLBACK) {
    title=L("Roll back the core?","回滚内核？");
    text=L("Restores the previous core. A running core restarts.","恢复上一版本内核；运行中的内核会重启。");
    snprintf(detail,sizeof(detail),"→ %s",data.core.previous[0]?data.core.previous:"--");
    button=L("Roll back","回滚");tone=DANGER;
  } else if(confirm_what==CF_ESIM) {
    title=L("Switch profile?","切换配置文件？");
    text=L("Mobile data stops for 10-30 s while the modem reads the card again.","基带重新读卡期间，移动数据中断约 10-30 秒。");
    snprintf(detail,sizeof(detail),"%s",chosen_name);
  } else if(confirm_what==CF_REBOOT) {
    title=L("Restart the device?","重启设备？");
    text=L("The firmware still uses the previous profile. The device restarts now.","固件仍在使用原来的配置文件，设备将立即重启。");
    button=L("Restart","重启");tone=DANGER;
  } else {
    title=L("Update geo data?","更新 GEO 数据？");
    text=L("Downloads the latest geo databases. A running core restarts.","下载最新的 GEO 数据库；运行中的内核会重启。");
    button=L("Update","更新");
  }
  dialog_card(112,244);
  box(28,128,264,32,"title",title);
  box(28,170,264,60,"wrap",text);
  if(detail[0])sbox(28,236,264,20,"head accent",detail);
  linkbox(24,284,128,56,"dismiss",L("Cancel","取消"),1,PLAIN);
  linkbox(168,284,128,56,"confirm",button,!blocked(),tone);
}

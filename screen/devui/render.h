/* DevUI HTML/CSS pages rendered by the upstream litehtml host.
 * Every string is bilingual through L(en, zh); the language follows the web panel.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
static char html[32768];
static size_t html_used;
static void add(const char *fmt, ...) {
  if (html_used >= sizeof(html) - 1) return;
  va_list args;
  va_start(args, fmt);
  int n = vsnprintf(html + html_used, sizeof(html) - html_used, fmt, args);
  va_end(args);
  if (n > 0) html_used += (size_t)n < sizeof(html) - html_used ? (size_t)n : sizeof(html) - html_used - 1;
}
static void escaped(const char *s) {
  for (; *s; s++) {
    switch (*s) {
      case '&': add("&amp;"); break;
      case '<': add("&lt;"); break;
      case '>': add("&gt;"); break;
      case '"': add("&quot;"); break;
      case '\'': add("&#39;"); break;
      default: add("%c", *s);
    }
  }
}
/* Escaped text cut on a UTF-8 boundary with "..." to fit `width` pixels at `size`.
 * Glyph advances add up (Unifont has no kerning), so each character is measured once. */
static void shortened(const char *text, int width, int size) {
  char label[512];snprintf(label,sizeof(label),"%s",text);
  if(html_view_text_width_px(label,size)>width) {
    int room=width-html_view_text_width_px("...",size),used=0;
    size_t n=0;
    while(label[n]) {
      size_t k=n+1;
      while(label[k]&&((unsigned char)label[k]&0xc0)==0x80)k++;
      char glyph[8];size_t len=k-n<sizeof(glyph)-1?k-n:sizeof(glyph)-1;
      memcpy(glyph,label+n,len);glyph[len]=0;
      int w=html_view_text_width_px(glyph,size);
      if(used+w>room)break;
      used+=w;n=k;
    }
    if(n>sizeof(label)-4)n=sizeof(label)-4;
    while(n>0&&((unsigned char)label[n]&0xc0)==0x80)n--;
    strcpy(label+n,"...");
  }
  escaped(label);
}
static void box(int x, int y, int w, int h, const char *cls, const char *label) {
  add("<div class=\"box %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">", cls, x,y,w,h);
  escaped(label); add("</div>");
}
/* A one-line box whose text is cut with "..." instead of clipped. */
static void sbox(int x, int y, int w, int h, const char *cls, const char *label) {
  add("<div class=\"box %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">", cls, x,y,w,h);
  shortened(label,w,16); add("</div>");
}
static void rect(int x, int y, int w, int h, const char *cls, const char *color) {
  add("<div class=\"box %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx%s%s\"></div>",cls,x,y,w,h,color?";background:":"",color?color:"");
}
/* CHOSEN marks the current choice of a group of buttons; it is not tappable. */
enum { PLAIN, PRIMARY, DANGER, CHOSEN };
static void linkbox(int x, int y, int w, int h, const char *id, const char *label, int enabled, int tone) {
  add("<a id=\"%s\" href=\"act:%s\" class=\"box btn %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx;line-height:%dpx\">",id,enabled&&tone!=CHOSEN?id:"none",
      tone==CHOSEN?"chosen":!enabled?"disabled":tone==PRIMARY?"primary":tone==DANGER?"danger":"",x,y,w,h,h-2);
  shortened(label,w-24,16); add("</a>");
}
/* A tappable list row: bold title, muted detail line and a chevron. */
static void open_row(int x, int y, int w, int h, const char *id, int enabled, const char *extra) {
  add("<a id=\"%s\" href=\"act:%s\" class=\"box row %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">",id,enabled?id:"none",enabled?extra:"off",x,y,w,h);
}
static void chevron(int h) { add("<div class=\"chev\" style=\"line-height:%dpx\">&#8250;</div>",h-2); }
static void nav_row(int x, int y, int w, int h, const char *id, const char *title, const char *detail, const char *detail_cls, int enabled) {
  /* Rows under 52 px centre their two lines; taller ones keep the stylesheet's. */
  char top[2][32]={"",""};
  if(h<52){snprintf(top[0],sizeof(top[0])," style=\"top:%dpx\"",(h-44)/2);snprintf(top[1],sizeof(top[1])," style=\"top:%dpx\"",(h-44)/2+22);}
  open_row(x,y,w,h,id,enabled,"");
  add("<div class=\"name\"%s>",top[0]);shortened(title,w-50,16);
  add("</div><div class=\"sub %s\"%s>",enabled?detail_cls:"",top[1]);shortened(detail,w-50,16);
  add("</div>");chevron(h);add("</a>");
}
/* Row with a label on the left and a value on the right, vertically centred. */
static void value_row(int x, int y, int w, int h, const char *id, const char *label, const char *value, int enabled) {
  open_row(x,y,w,h,id,enabled,"");
  add("<div class=\"box muted\" style=\"left:14px;top:%dpx;width:%dpx;height:20px\">",(h-22)/2,w/2-14);escaped(label);
  add("</div><div class=\"box head right\" style=\"left:%dpx;top:%dpx;width:%dpx;height:20px\">",w/2-10,(h-22)/2,w/2-24);
  shortened(value,w/2-24,16);add("</div>");chevron(h);add("</a>");
}
static const char *summary(void) {
  return !data.stamp ? L("Loading status","正在加载状态") : stale() ? L("Status is stale","状态已过期") :
    applying(0) ? L("Applying changes","正在应用更改") : !data.has ? L("No configuration","尚未配置") :
    data.error ? L("Tunnel error","隧道错误") : !data.enabled ? L("Off","已关闭") :
    data.connected ? L("Recent handshake","握手正常") : L("Waiting for handshake","等待握手");
}
/* Colour of the summary above: red for trouble, green when connected, amber in between. */
static const char *summary_class(void) {
  return !data.stamp || applying(0) ? "warn" : stale() || data.error ? "bad" :
    !data.has || !data.enabled ? "muted" : data.connected ? "good" : "warn";
}
static const char *tone_color(const char *c) {
  return !strcmp(c,"good") ? "#4ade9a" : !strcmp(c,"bad") ? "#ff8a8a" : !strcmp(c,"warn") ? "#fbbf5a" : "#5d6b82";
}
static const char *summary_color(void) { return tone_color(summary_class()); }
static const char *active_profile(void) {
  for (int i=0;i<data.np;i++) if(data.profiles[i].active) return data.profiles[i].name;
  return L("None selected","未选择");
}
static int selected_count(void) {
  int n=0;for(int i=0;i<data.n;i++) n+=data.rows[i].selected;return n;
}
static void header(const char *title) {
  add("<div class=\"box title\" style=\"left:16px;top:16px;width:200px;height:32px\">");shortened(title,200,24);add("</div>");
  linkbox(220,8,92,48,"back",L("‹ Back","‹ 返回"),1,PLAIN);
  rect(0,64,W,1,"line",NULL);
}
static void dot(int x, int y, const char *color) {
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:10px;height:10px;border-radius:5px;background:%s\"></div>",x,y,color);
}
static void battery_badge(int x, int y, int large) {
  int percent = data.battery < 0 ? 0 : data.battery > 100 ? 100 : data.battery;
  const char *color = data.battery < 0 ? "#8e9bb3" : percent <= 20 ? "#ff8a8a" : "#4ade9a";
  int w=large?62:30, h=large?30:16;
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx;border:2px solid %s;border-radius:%dpx\"></div>",x,y,w,h,color,large?6:3);
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:3px;height:%dpx;background:%s\"></div>",x+w+1,y+h/3,h-2*(h/3),color);
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx;background:%s;border-radius:%dpx\"></div>",x+4,y+4,(w-8)*percent/100,h-8,color,large?3:1);
  char label[24];
  if(data.battery<0)snprintf(label,sizeof(label),"--%%");else snprintf(label,sizeof(label),"%d%%",percent);
  if(large)box(x+w+14,y-3,120,36,"big",label);else box(x+w+10,y-2,60,20,"head",label);
}
/* Label, right-aligned value and an optional usage bar (percent < 0: no bar). */
static void stat_row(int y, const char *label, const char *value, double percent) {
  box(26,y,130,20,"muted",label);box(156,y,138,20,"head right",value);
  if(percent<0)return;
  int fill=(int)(percent<0?0:percent>100?268:percent*268/100);
  rect(26,y+26,268,6,"bar",NULL);
  if(fill>0)rect(26,y+26,fill,6,"fill",percent>=85?"#ff8a8a":percent>=60?"#fbbf5a":"#7fa9ff");
}
static void dialog_card(int y, int h) {
  rect(0,65,W,H-65,"shade",NULL);
  add("<div class=\"box card\" style=\"left:12px;top:%dpx;width:296px;height:%dpx\"></div>",y,h);
}
#include "pages.h"
static uint16_t pixels[W * H];
/* The current page as HTML. Cheap next to laying it out and drawing it. */
static void build_html(void) {
  html_used=0;html[0]=0;
  add("<!DOCTYPE html><html lang=\"%s\"><head><meta charset=\"UTF-8\"><style>%s</style></head><body>",L("en","zh"),style_asset);
  char tmp[256],rx[32],tx[32];
  if (dialog) {
    header(page_title());
    if (dialog==5) render_confirm();
    else if (dialog==6) render_sub_dialog();
    else if (dialog==1 || dialog==4) {
      dialog_card(112,244);
      box(28,128,264,32,"title",dialog==1?L("Disable WireGuard?","关闭 WireGuard？"):L("Switch profile?","切换配置？"));
      box(28,170,264,60,"wrap",dialog==1?L("Devices will return to the normal network.","设备将恢复为普通网络。"):
          data.enabled?L("Tunnel traffic will briefly pause. Routing choices stay saved.","隧道流量会短暂中断，设备分流选择保持不变。"):
          L("Select this profile. WireGuard will remain off.","选中此配置，WireGuard 仍保持关闭。"));
      if(dialog==4) { add("<div class=\"box head accent\" style=\"left:28px;top:236px;width:264px;height:20px\">");shortened(chosen_name,264,16);add("</div>"); }
      linkbox(24,284,128,56,"dismiss",L("Cancel","取消"),1,PLAIN);
      linkbox(168,284,128,56,"confirm",dialog==1?L("Disable","关闭"):L("Switch","切换"),!blocked(),dialog==1?DANGER:PRIMARY);
    } else if(dialog==2) {
      dialog_card(88,332);
      box(28,106,264,32,"title",L("Unsaved changes","有未保存的更改"));
      box(28,146,264,40,"wrap muted",L("Device routing choices have not been saved.","设备分流的选择尚未保存。"));
      linkbox(24,200,272,56,"saveback",L("Save and return","保存并返回"),!blocked(),PRIMARY);
      linkbox(24,268,272,56,"discard",L("Discard changes","放弃更改"),1,DANGER);
      linkbox(24,336,272,56,"dismiss",L("Keep editing","继续编辑"),1,PLAIN);
    } else {
      dialog_card(112,244);
      box(28,128,264,32,"title bad",L("Action failed","操作失败"));
      box(28,170,264,40,"wrap",notice);
      /* The agent's own message, when it gave one (English). */
      box(28,214,264,60,"wrap muted",failure_text[0]?failure_text:L("Reload status before trying again.","请先刷新状态再重试。"));
      /* WireGuard pages go back to WireGuard; the others reload where they are. */
      linkbox(24,284,272,56,"refresh",page==WG||page==PROFILES||page==DEVICES?L("Refresh and back","刷新并返回"):L("Refresh","刷新"),1,PRIMARY);
    }
  } else if(page==MENU) {
    box(16,10,180,32,"title","OpenUI");
    box(16,40,180,20,"muted",data.toolbox.has?L("Pocket OS Toolbox","随身系统工具箱"):L("Device control","设备控制"));
    linkbox(208,8,104,48,"stock",L("Stock UI","原厂界面"),1,PLAIN);
    add("<div class=\"box card\" style=\"left:12px;top:72px;width:296px;height:56px\"></div>");
    battery_badge(28,92,0);
    rect(108,84,1,32,"line",NULL);
    /* The active proxy backend when the agent reports it, else WireGuard as before. */
    const struct backend_info *bk=&data.backend;
    const char *cls=bk->has?proxy_class():summary_class();
    dot(122,83,tone_color(cls));
    if(data.toolbox.has) {
      char d_rate[24],u_rate[24],spd_line[96];
      format_rate(d_rate,sizeof(d_rate),data.toolbox.kbps_down);
      format_rate(u_rate,sizeof(u_rate),data.toolbox.kbps_up);
      snprintf(spd_line,sizeof(spd_line),"%s · ↓%s ↑%s",
               !bk->has||bk->active==BK_WG?"WG":bk->active==BK_CLASH?"Clash":L("Direct","直连"),d_rate,u_rate);
      sbox(138,78,162,20,"muted",spd_line);
    } else {
      box(138,78,160,20,"muted",!bk->has||bk->active==BK_WG?"WireGuard":bk->active==BK_CLASH?"Clash":L("Proxy","代理"));
    }
    add("<div class=\"box head %s\" style=\"left:122px;top:100px;width:178px;height:20px\">",cls);shortened(bk->has?proxy_summary():summary(),176,16);add("</div>");
    const int pages[] = {OVERVIEW,PROXY,WG,CLASH,ESIM,UPLINK,WIFI_PAGE,SIGNAL_PAGE,USB_PAGE,POWER_PAGE,BOOSTER_PAGE,PROFILES,DEVICES};
    const char *ids[] = {"overview","proxy","wireguard","clash","esim","uplink","wifi","signal","usb","power","booster","profiles","devices"};
    const char *titles[] = {
      L("Overview","概览"),L("Proxy","代理"),"WireGuard","Clash","eSIM",L("Wi-Fi relay","Wi-Fi 中继"),
      L("Wi-Fi","Wi-Fi"),L("5G Signal","5G 信号"),L("USB & TTL","USB / TTL"),
      L("Battery","电池充电"),L("Booster","加速清理"),
      L("Saved profiles","已保存配置"),L("Device routing","设备分流")
    };
    char wifi_det[64],sig_det[64],usb_det[64],pwr_det[64],bst_det[64];
    snprintf(wifi_det,sizeof(wifi_det),L("%s · %llu clients","%s · %llu 台在线"),
             (data.toolbox.wifi_2g_on&&data.toolbox.wifi_5g_on)?"2.4G+5G":data.toolbox.wifi_5g_on?"5G":data.toolbox.wifi_2g_on?"2.4G":L("Off","已关"),
             data.toolbox.wifi_clients);
    snprintf(sig_det,sizeof(sig_det),"%s %s (%llddBm)",
             data.toolbox.radio[0]?data.toolbox.radio:"5G",data.toolbox.band[0]?data.toolbox.band:"Auto",data.toolbox.rsrp);
    if(data.toolbox.ttl_val)snprintf(usb_det,sizeof(usb_det),"%s · TTL %llu",data.toolbox.usb_mode[0]?data.toolbox.usb_mode:"rndis",data.toolbox.ttl_val);
    else snprintf(usb_det,sizeof(usb_det),"%s%s",data.toolbox.usb_mode[0]?data.toolbox.usb_mode:"rndis",data.toolbox.usb_adb?" + ADB":"");
    if(data.toolbox.chg_limit_on)snprintf(pwr_det,sizeof(pwr_det),L("Limit %llu%% · %lldmA","限充 %llu%% · %lldmA"),data.toolbox.chg_stop,data.toolbox.batt_ma);
    else snprintf(pwr_det,sizeof(pwr_det),"%s · %lldmA",data.toolbox.chg_enabled?L("Charging","充电中"):L("Paused","已停充"),data.toolbox.batt_ma);
    snprintf(bst_det,sizeof(bst_det),L("RAM %.0f%% · %llu bloat","内存 %.0f%% · %llu 冗余"),data.memory>=0?data.memory:0.0,data.toolbox.bloat_cnt);
    const char *details[] = {
      L("Battery, uptime and load","电池、运行时间与系统负载"),
      !bk->has||bk->active==BK_OFF?L("Off / WireGuard / Clash","关闭 / WireGuard / Clash"):bk->active==BK_WG?L("Active: WireGuard","当前：WireGuard"):L("Active: Clash","当前：Clash"),
      L("Tunnel status and on / off","隧道状态与开关"),L("Nodes, subscriptions and core","节点、订阅与内核"),
      esim_summary(),uplink_summary(),
      wifi_det,sig_det,usb_det,pwr_det,bst_det,
      L("Switch between up to 5 configs","在最多 5 个配置之间切换"),L("Choose WireGuard or direct","选择走 WireGuard 或直连")
    };
    int vis_idx[13], vis_cnt=0;
    for(int i=0;i<13;i++) if(visible_page(pages[i])) vis_idx[vis_cnt++]=i;
    if(vis_cnt<=4) {
      for(int r=0;r<vis_cnt;r++) {
        int i=vis_idx[r];
        nav_row(12,140+68*r,296,60,ids[i],titles[i],details[i],"",1);
      }
    } else {
      int show=vis_cnt>9?9:vis_cnt;
      for(int r=0;r<show;r++) {
        int i=vis_idx[r];
        int col=r%3, row_i=r/3;
        int tx=12+col*101, ty=136+row_i*92;
        add("<a id=\"%s\" href=\"act:%s\" class=\"box tile\" style=\"left:%dpx;top:%dpx;width:94px;height:84px\">",ids[i],ids[i],tx,ty);
        add("<div class=\"box head\" style=\"left:8px;top:14px;width:78px;height:20px\">");shortened(titles[i],78,16);add("</div>");
        add("<div class=\"box muted\" style=\"left:8px;top:44px;width:78px;height:20px\">");shortened(details[i],78,16);add("</div></a>");
      }
    }
    box(16,418,292,20,"muted",L("Power key: tap to sleep / wake.","电源键短按：息屏 / 唤醒"));
    if(settings_sleep) snprintf(tmp,sizeof(tmp),L("Screen turns off after %d s idle.","空闲 %d 秒后熄屏"),settings_idle);
    else snprintf(tmp,sizeof(tmp),L("Stock UI returns after %d s idle.","空闲 %d 秒后返回原厂界面"),settings_idle);
    box(16,440,292,20,"muted",tmp);
  } else if(page==WG) {
    header("WireGuard");
    add("<div class=\"box card\" style=\"left:12px;top:76px;width:296px;height:92px\"></div>");
    dot(28,91,summary_color());
    snprintf(tmp,sizeof(tmp),"head %s",strcmp(summary_class(),"muted")?summary_class():"");
    box(46,86,250,20,tmp,summary());
    if(!data.has)snprintf(tmp,sizeof(tmp),"%s",L("Import a config in OpenUI","请先在 OpenUI 中导入配置"));
    else if(data.enabled&&data.handshake)snprintf(tmp,sizeof(tmp),L("Last handshake: %llu s ago","上次握手：%llu 秒前"),(unsigned long long)time(NULL)>data.handshake?(unsigned long long)time(NULL)-data.handshake:0);
    else snprintf(tmp,sizeof(tmp),"%s",data.enabled?L("Waiting for the server","正在等待服务器响应"):L("Devices use normal internet","设备使用普通网络"));
    box(28,112,268,20,"muted",tmp);
    box(28,136,268,20,"muted",L("Internet access not verified","未验证互联网连通性"));
    amount(rx,sizeof(rx),data.rx);amount(tx,sizeof(tx),data.tx);
    add("<div class=\"box card\" style=\"left:12px;top:180px;width:142px;height:64px\"></div><div class=\"box card\" style=\"left:166px;top:180px;width:142px;height:64px\"></div>");
    box(26,188,120,20,"muted",L("↓ Downloaded","↓ 下载"));box(180,188,120,20,"muted",L("↑ Uploaded","↑ 上传"));
    box(26,208,124,32,"title",rx);box(180,208,124,32,"title",tx);
    linkbox(12,256,296,56,"toggle",data.enabled?L("Disable WireGuard","关闭 WireGuard"):
            data.backend.has&&data.backend.active==BK_CLASH?L("Switch to WireGuard","切换到 WireGuard"):L("Enable WireGuard","开启 WireGuard"),
            !blocked()&&data.has,data.enabled?DANGER:PRIMARY);
    value_row(12,324,296,50,"profiles",L("Profile","当前配置"),active_profile(),reachable(PROFILES));
    snprintf(tmp,sizeof(tmp),"%d / %d",selected_count(),data.n);
    value_row(12,382,296,50,"devices",L("Device routing","设备分流"),tmp,reachable(DEVICES));
    if(notice[0])box(16,446,292,20,"bad",notice);
    else box(16,446,292,20,"muted",data.all?L("New devices: WireGuard","新设备：走 WireGuard"):L("New devices: normal network","新设备：走普通网络"));
  } else if(page==PROFILES) {
    header(L("Saved profiles","已保存配置"));
    snprintf(tmp,sizeof(tmp),L("%d / 5 saved - tap to switch","已保存 %d / 5 个，点击切换"),data.np);
    box(16,76,292,20,"muted",tmp);
    for(int i=0;i<data.np;i++) {
      char id[16];snprintf(id,sizeof(id),"p%d",i);
      int active=data.profiles[i].active;
      open_row(12,104+i*60,296,52,id,active||!blocked(),active?"current":"");
      add("<div class=\"box head\" style=\"left:14px;top:15px;width:%dpx;height:20px\">",active?200:236);shortened(data.profiles[i].name,active?200:236,16);add("</div>");
      if(active)add("<div class=\"box badge\" style=\"right:12px;top:14px;width:56px;height:22px\">%s</div>",L("Active","当前"));
      else chevron(52);
      add("</a>");
    }
    if(!data.np)box(28,150,264,60,"wrap muted",L("Import profiles in the OpenUI webpage first.","请先在 OpenUI 网页中导入配置。"));
    if(stale())box(16,420,292,40,"wrap bad",L("Status is stale. Controls are disabled.","状态已过期，控制已停用。"));
    else {
      box(16,420,292,20,"muted",L("Switching keeps device routing.","切换配置不会改变设备分流。"));
      box(16,442,292,20,"muted",L("Manage names and keys in OpenUI.","名称与密钥请在 OpenUI 中管理。"));
    }
  } else if(page==DEVICES) {
    header(L("Device routing","设备分流"));
    box(16,72,292,20,"muted",draft.all?L("New devices: WireGuard","新设备：走 WireGuard"):L("New devices: normal network","新设备：走普通网络"));
    box(16,92,292,20,stale()?"bad":"muted",stale()?L("Status is stale; changes disabled.","状态已过期，无法修改。"):
        data.enabled?L("Checked: WireGuard. Others: direct.","勾选的走 WireGuard，其余直连。"):L("Changes take effect when WG is on.","WireGuard 开启后生效。"));
    for(int i=0;i<4&&offset+i<draft.n;i++) {
      struct row *r=&draft.rows[offset+i];
      int dup=!strcmp(r->name,"-");
      for(int j=0;j<draft.n;j++)if(j!=offset+i&&!strcmp(r->name,draft.rows[j].name))dup=1;
      int y=120+i*60;
      add("<a id=\"d%d\" href=\"act:d%d\" class=\"box row%s\" style=\"left:12px;top:%dpx;width:296px;height:54px\">",i,i,r->selected?" current":"",y);
      add("<div class=\"box check%s\" style=\"left:12px;top:15px;width:24px;height:24px\">%s</div>",r->selected?" on":"",r->selected?"&#10003;":"");
      add("<div class=\"name\" style=\"left:48px;right:10px;top:7px\">");
      shortened(!strcmp(r->name,"-")?L("Unknown device","未知设备"):r->name,234,16);
      add("</div><div class=\"sub\" style=\"left:48px;right:10px;top:29px\"><span class=\"%s\">&#9679;</span> ",r->online?"good":"muted");
      snprintf(tmp,sizeof(tmp),"%s  %s%s%s",r->online?L("Online","在线"):L("Offline","离线"),strcmp(r->ip,"-")?r->ip:"",dup?"  ":"",dup?r->mac+9:"");
      escaped(tmp);add("</div></a>");
    }
    if(!draft.n)box(28,180,264,40,"wrap muted center",L("No devices found","未发现设备"));
    linkbox(12,364,100,48,"prev",L("‹ Prev","‹ 上页"),offset>0,PLAIN);
    linkbox(208,364,100,48,"next",L("Next ›","下页 ›"),offset+4<draft.n,PLAIN);
    snprintf(tmp,sizeof(tmp),"%d / %d",offset/4+1,draft.n?(draft.n+3)/4:1);box(112,378,96,20,"muted center",tmp);
    linkbox(12,420,142,52,"back",L("Cancel","取消"),1,PLAIN);
    linkbox(166,420,142,52,"save",pending?L("Applying","正在应用"):L("Save","保存"),dirty&&!blocked(),PRIMARY);
  } else if(page==OVERVIEW) {
    header(L("Overview","概览"));
    add("<div class=\"box card\" style=\"left:12px;top:76px;width:296px;height:96px\"></div>");
    box(28,86,120,20,"muted",L("Battery","电池"));
    if(data.temperature!=-999)snprintf(tmp,sizeof(tmp),"%.1f °C",data.temperature/10.0);else strcpy(tmp,"-- °C");
    box(176,86,118,20,"muted right",tmp);
    battery_badge(28,124,1);
    add("<div class=\"box card\" style=\"left:12px;top:184px;width:296px;height:138px\"></div>");
    if(data.uptime>=86400)snprintf(tmp,sizeof(tmp),L("%llud %lluh %llum","%llu 天 %llu 时 %llu 分"),data.uptime/86400,data.uptime/3600%24,data.uptime/60%60);
    else snprintf(tmp,sizeof(tmp),L("%lluh %llum","%llu 时 %llu 分"),data.uptime/3600,data.uptime/60%60);
    stat_row(198,L("Uptime","运行时间"),tmp,-1);
    snprintf(tmp,sizeof(tmp),"%.1f%%",data.cpu);stat_row(230,"CPU",tmp,data.cpu);
    if(data.memory>=0)snprintf(tmp,sizeof(tmp),"%.1f%%",data.memory);else strcpy(tmp,"--");
    stat_row(274,L("Memory","内存"),tmp,data.memory);
    /* With a Wi-Fi uplink reported, its row joins and the note shrinks to one line. */
    int relay=data.uplink.has,rh=relay?56:60;
    if(data.backend.has&&visible_page(PROXY))nav_row(12,334,296,rh,"proxy",L("Proxy","代理"),proxy_summary(),proxy_class(),1);
    else nav_row(12,334,296,rh,"wireguard","WireGuard",summary(),summary_class(),visible_page(WG));
    if(relay)nav_row(12,398,296,56,"uplink",L("Wi-Fi relay","Wi-Fi 中继"),uplink_summary(),uplink_class(),1);
    box(16,relay?458:412,292,relay?20:40,stale()?"wrap bad":"wrap muted",stale()?L("Status unavailable / stale","状态不可用或已过期"):L("Live status from the OpenUI agent","实时状态来自 OpenUI Agent"));
  }
  else if(page==PROXY) render_proxy();
  else if(page==CLASH) render_clash();
  else if(page==NODES) render_nodes();
  else if(page==GROUP) render_group();
  else if(page==SUBS) render_subs();
  else if(page==CORE) render_core();
  else if(page==DIAG) render_diag();
  else if(page==ESIM) render_esim();
  else if(page==UPLINK) render_uplink();
  else if(page==WIFI_PAGE) render_wifi();
  else if(page==SIGNAL_PAGE) render_signal();
  else if(page==USB_PAGE) render_usb();
  else if(page==POWER_PAGE) render_power();
  else if(page==BOOSTER_PAGE) render_booster();
  add("</body></html>");
}
static void draw_html(struct drm_buf *b) {
  static int initialized;
  if (!initialized) {
    html_view_init(pixels,W,H,W,0,NULL);
    initialized=1;
  }
  html_view_render_to(pixels,html);
  for(int y=0;y<H;y++)memcpy((uint8_t*)b->map+y*b->pitch,pixels+y*W,W*2);
}
static void render(struct drm_buf *b) {
  build_html();
  draw_html(b);
}
/* The page last drawn into a frame. Equal HTML draws equal pixels (the rotation is fixed
 * and the backlight keeps the panel's image), so a status poll that changes nothing on
 * the page costs no layout, drawing or flip. */
static char drawn_html[sizeof html];
static int drawn_valid;
/* Draws the page into `b` unless it is the page last drawn; returns whether it drew. */
static int render_changed(struct drm_buf *b, int force) {
  build_html();
  if (!force && drawn_valid && !strcmp(html,drawn_html)) return 0;
  draw_html(b);
  memcpy(drawn_html,html,html_used+1);
  drawn_valid=1;
  return 1;
}

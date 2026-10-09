/* Whitelisted DevUI actions. No scripts, passwords or keys in the UI.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
/* Node pages read a page of the agent's cached view; indices are only valid with the
 * generation they came with. */
static void fetch_groups(void) {
  char req[48],reply[4096];
  snprintf(req,sizeof(req),"groups %d\n",group_offset);
  if(exchange(req,reply,sizeof(reply))!=0||!parse_groups(reply,&groups))groups.state=-1;
  else group_offset=groups.offset;
  page_fetch_at=monotonic_ms()+(groups.state==0?700:2000);
  need_draw=1;
}
static void fetch_nodes(void) {
  char req[96],reply[4096];
  snprintf(req,sizeof(req),"nodes %llu %d %d\n",open_gen,open_group,node_offset);
  page_fetch_at=monotonic_ms()+2000;
  need_draw=1;
  if(exchange(req,reply,sizeof(reply))!=0){nodes.ok=0;return;}
  if(!strncmp(reply,"ERR Stale",9)) {
    /* An open dialog keeps its message and the page under it; the next fetch reloads. */
    if(dialog)return;
    snprintf(notice,sizeof(notice),"%s",L("The node list changed. Reloaded.","节点列表已变化，已重新加载。"));
    if(page==GROUP)page=NODES;
    fetch_groups();
    return;
  }
  char name[193];snprintf(name,sizeof(name),"%s",nodes.name);
  if(parse_nodes(reply,&nodes)&&nodes.gen==open_gen&&nodes.group==open_group)node_offset=nodes.offset;
  else{nodes.ok=0;snprintf(nodes.name,sizeof(nodes.name),"%s",name);}
}
static void refresh_page(void) {
  if(page==NODES)fetch_groups();
  else if(page==GROUP)fetch_nodes();
}
static void go(int p) {
  page=p;notice[0]=0;need_draw=1;
  if(p==NODES)fetch_groups();
}
static int parent(int p) {
  switch(p) {
  case PROFILES: case DEVICES: return visible_page(WG)?WG:MENU;
  case NODES: case SUBS: case CORE: return CLASH;
  case GROUP: return NODES;
  case DIAG: return (diag_from==PROXY||diag_from==CLASH)&&reachable(diag_from)?diag_from:MENU;
  default: return MENU;
  }
}
static void back(void) {
  if(dirty&&!pending)dialog=2;
  else { int to=parent(page);dirty=0;dialog=0;go(to); }
  need_draw=1;
}
static void enter_devices(void) {
  draft=data;
  for(int i=0;i<draft.n;i++)original_selection[i]=draft.rows[i].selected;
  offset=0;dirty=0;page=DEVICES;
}
static void ask(int what, int target, int job, unsigned long long rev, const char *req) {
  snprintf(confirm_req,sizeof(confirm_req),"%s",req);
  confirm_what=what;confirm_target=target;confirm_job=job;confirm_rev=rev;dialog=5;
}
/* Switching on names the revision of the backend being turned on; off, the revision of
 * the one shown as on and that backend, which the agent checks is still the one on. */
static void confirm_backend(int target) {
  const struct backend_info *b=&data.backend;
  unsigned long long rev=b->rev[target==BK_OFF?b->active==BK_CLASH:target==BK_CLASH];
  const char *to=target==BK_WG?"wireguard":target==BK_CLASH?"clash":b->active==BK_CLASH?"off clash":"off wireguard";
  char req[64];
  snprintf(req,sizeof(req),"backend %llu %s\n",rev,to);
  ask(CF_BACKEND,target,JOB_BACKEND,rev,req);
}
static int digit_id(const char *a, char c, char max) { return a[0]==c&&a[1]>='0'&&a[1]<=max&&!a[2]?a[1]-'0':-1; }
static void dialog_tap(const char *a) {
  if(!strcmp(a,"dismiss"))dialog=0;
  else if(!strcmp(a,"confirm")&&!blocked()) {
    if(dialog==1)submit(1,0);
    else if(dialog==4)submit(2,0);
    else if(dialog==5)send_job(confirm_req,confirm_job,confirm_rev);
  } else if(!strcmp(a,"saveback")&&dialog==2&&!blocked()) {
    return_after_save=1;submit(0,0);
  } else if(!strcmp(a,"discard")&&dialog==2) {
    dirty=0;page=parent(DEVICES);dialog=0;
  } else if(!strcmp(a,"refresh")&&dialog==3) {
    dialog=0;notice[0]=0;failure_text[0]=0;
    /* Profiles and devices go back to WireGuard, or to the menu without it. */
    if(page==WG||page==PROFILES||page==DEVICES){dirty=0;if(page!=WG)page=parent(page);}
    else refresh_page();
  } else if(dialog==6&&!blocked()&&!strcmp(a,"activate")) {
    char req[64];snprintf(req,sizeof(req),"csub %llu %llu\n",chosen_sub_rev,chosen_sub);
    send_job(req,JOB_SUB,chosen_sub_rev);
  } else if(dialog==6&&!blocked()&&!strcmp(a,"srefresh")) {
    char req[64];snprintf(req,sizeof(req),"crefresh %llu\n",chosen_sub);
    send_job(req,JOB_REFRESH,0);
  } else if(!strcmp(a,"back"))dialog=0;
}
static void tap(int x,int y) {
  char action[96];
  snprintf(action,sizeof(action),"%s",html_view_click((float)x,(float)y));
  if(strncmp(action,"act:",4))return;
  const char *a=action+4;
  if(!strcmp(a,"none"))return;
  need_draw=1;
  if(dialog){dialog_tap(a);return;}
  if(!strcmp(a,"stock")) { quitting=1;return; }
  if(!strcmp(a,"back")) { back();return; }
  static const struct { const char *id; int page; } links[]={
    {"wireguard",WG},{"overview",OVERVIEW},{"profiles",PROFILES},{"proxy",PROXY},
    {"clash",CLASH},{"nodes",NODES},{"subs",SUBS},{"core",CORE}};
  for(size_t i=0;i<sizeof(links)/sizeof(links[0]);i++)
    if(!strcmp(a,links[i].id)) { if(reachable(links[i].page)){group_offset=0;go(links[i].page);} return; }
  if(!strcmp(a,"devices")) { if(reachable(DEVICES))enter_devices();return; }
  if(!strcmp(a,"diag")) { if(reachable(DIAG)){diag_from=page;diag_offset=0;go(DIAG);} return; }
  if(!strcmp(a,"active")&&page==PROXY) {
    int to=data.backend.active==BK_WG?WG:data.backend.active==BK_CLASH?CLASH:-1;
    if(to>=0&&reachable(to))go(to);
    return;
  }
  int i;
  if(!strcmp(a,"toggle")&&page==WG&&!blocked()&&data.has) {
    if(data.enabled)dialog=1;
    else if(data.backend.has&&data.backend.active==BK_CLASH)confirm_backend(BK_WG);
    else submit(1,1);
  } else if(page==PROFILES&&(i=digit_id(a,'p','4'))>=0&&!blocked()) {
    if(i<data.np&&!data.profiles[i].active) {
      chosen_profile=data.profiles[i].id;chosen_revision=data.rev;
      snprintf(chosen_name,sizeof(chosen_name),"%s",data.profiles[i].name);
      dialog=4;
    }
  } else if(page==DEVICES) {
    if(!strcmp(a,"prev")&&offset>0)offset-=4;
    else if(!strcmp(a,"next")&&offset+4<draft.n)offset+=4;
    else if(!strcmp(a,"save")&&dirty&&!blocked()) {
      return_after_save=0;submit(0,0);
    } else if((i=digit_id(a,'d','3'))>=0&&!blocked()) {
      i+=offset;
      if(i<draft.n) {
        draft.rows[i].selected=!draft.rows[i].selected;
        dirty=0;
        for(int j=0;j<draft.n;j++)if(draft.rows[j].selected!=original_selection[j])dirty=1;
      }
    }
  } else if(page==PROXY&&!strncmp(a,"bk",2)&&a[2]>='0'&&a[2]<='2'&&!a[3]) {
    const struct backend_info *b=&data.backend;
    int t=a[2]-'0';
    if(b->has&&!blocked()&&(b->active!=t||b->conflict)&&(t==BK_OFF||b->ready[t-1]))confirm_backend(t);
  } else if(page==CLASH) {
    const struct clash_info *c=&data.clash;
    const struct backend_info *b=&data.backend;
    if(!strcmp(a,"ctoggle")&&!blocked()&&c->has&&b->has) {
      if(c->enabled){if(b->active==BK_CLASH)confirm_backend(BK_OFF);}
      else if(b->ready[1]&&b->active==BK_WG)confirm_backend(BK_CLASH);
      else if(b->ready[1]) {
        char req[64];snprintf(req,sizeof(req),"backend %llu clash\n",b->rev[1]);
        send_job(req,JOB_BACKEND,b->rev[1]);
      }
    } else if((i=digit_id(a,'m','2'))>=0&&!blocked()&&c->has) {
      static const char *modes[]={"rule","global","direct"};
      if(strcmp(c->mode,modes[i])) {
        char req[64];snprintf(req,sizeof(req),"cmode %llu %s\n",c->rev,modes[i]);
        send_job(req,JOB_MODE,c->rev);
      }
    }
  } else if(page==NODES) {
    if(!strcmp(a,"prev")&&group_offset>0){group_offset-=4;fetch_groups();}
    else if(!strcmp(a,"next")&&group_offset+4<groups.total){group_offset+=4;fetch_groups();}
    else if((i=digit_id(a,'g','3'))>=0&&groups.state==1&&i<groups.n) {
      const struct group_row *r=&groups.rows[i];
      open_group=r->index;open_gen=groups.gen;node_offset=r->now>0?r->now/4*4:0;
      memset(&nodes,0,sizeof(nodes));
      snprintf(nodes.name,sizeof(nodes.name),"%s",r->name);
      go(GROUP);fetch_nodes();
    }
  } else if(page==GROUP) {
    if(!strcmp(a,"prev")&&node_offset>0){node_offset-=4;fetch_nodes();}
    else if(!strcmp(a,"next")&&node_offset+4<nodes.total){node_offset+=4;fetch_nodes();}
    else if(!strcmp(a,"test")&&nodes.ok&&!blocked()) {
      char req[96];snprintf(req,sizeof(req),"delay %llu %d\n",open_gen,open_group);
      send_job(req,JOB_DELAY,0);
    } else if((i=digit_id(a,'n','3'))>=0&&nodes.ok&&nodes.selectable&&!blocked()&&i<nodes.n&&nodes.rows[i].index!=nodes.now) {
      char req[96];snprintf(req,sizeof(req),"select %llu %d %d\n",open_gen,open_group,nodes.rows[i].index);
      send_job(req,JOB_SELECT,0);
    }
  } else if(page==SUBS) {
    if((i=digit_id(a,'s','4'))>=0&&!stale()&&i<data.nsubs) {
      chosen_sub=data.subs[i].id;chosen_sub_rev=data.clash.rev;dialog=6;
    }
  } else if(page==CORE) {
    const struct core_info *c=&data.core;
    int idle=core_idle();
    if(!strcmp(a,"ccheck")&&idle)send_job("core check\n",JOB_CORE,0);
    else if(!strcmp(a,"cupdate")&&core_update_ok()) {
      char req[64];snprintf(req,sizeof(req),"core update %s\n",c->latest);
      ask(CF_UPDATE,0,JOB_CORE,0,req);
    } else if(!strcmp(a,"crollback")&&idle&&c->previous[0])ask(CF_ROLLBACK,0,JOB_CORE,0,"core rollback\n");
    else if(!strcmp(a,"cgeo")&&idle)ask(CF_GEO,0,JOB_CORE,0,"core geodata\n");
  } else if(page==DIAG) {
    if(!strcmp(a,"drun")&&!blocked()&&data.diag.has&&!data.diag.running)send_job("diag\n",JOB_DIAG,0);
    else if(!strcmp(a,"prev")&&diag_offset>0)diag_offset-=4;
    else if(!strcmp(a,"next")&&diag_offset+4<data.diag.n)diag_offset+=4;
  }
}

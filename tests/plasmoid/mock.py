#!/usr/bin/python3
"""Private-bus fixture. Never connect this mock to the user's session bus."""
import json, os, sys, time
from gi.repository import Gio, GLib
NAME = 'io.github.StantonMatt.DesktopIdleStatus'
PATH = '/io/github/StantonMatt/DesktopIdleStatus'
IFACE = NAME + '1'
state = sys.argv[1]
notice_id = 0
unclaimed_notices = set()
now = int(time.time())
def window(app, icon, caption, since, ident):
    return dict(appName=app, iconName=icon, caption=caption, since=since, internalId=ident, appId=app.lower())
firefox = window('Firefox', 'firefox', 'Lo-fi Radio – YouTube', now-1980, 'firefox-id')
chrome = window('Google Chrome', 'google-chrome', 'Weekly sync – Google Meet', now-2940, 'chrome-id')
haruna = window('Haruna', 'haruna', 'big_buck_bunny_1080p.mkv', now-2100, 'haruna-id')
def interval(row, start, end):
    return dict(row, start=start, end=end)
history = [interval(firefox, now-24000, now-4200), interval(haruna, now-86400, now-81900), interval(chrome, now-90000, now-87300)]
props = dict(State='ready', ExactAttribution=True, UnavailableCode='', UnavailableReason='', ScreensaverTimeout=600,
             Blockers=[], BlockedUnattributed=False, LockSleepBlockers=[], TimeoutConflicts=[], RunningSince=0, ScreensaverOffReason='')
if state.startswith('blocked') or state == 'long':
    props['State']='blocked'; props['Blockers']=[firefox] if state=='blocked-one' else [chrome, haruna, firefox]
if state=='unidentified':
    props.update(State='blocked', ExactAttribution=False, BlockedUnattributed=True, UnavailableCode='bridge-missing', UnavailableReason='KWin bridge missing or not loaded')
    history.insert(0, interval(window('Unidentified window','preferences-system-windows','',0,'unattributed'), now-25000, now-6400))
if state in ['late', 'late-fractional', 'late-seconds', 'late-power-lock']:
    props['TimeoutConflicts']=[dict(setting=s, seconds=t, kcm='kcm_screenlocker' if s=='lock' else 'kcm_powerdevilprofilesconfig') for s,t in [('lock',300),('dim',300),('screen-off',600)]]
    if state == 'late-fractional':
        props['TimeoutConflicts'][0]['seconds'] = 89
    if state == 'late-seconds':
        props['TimeoutConflicts'][0]['seconds'] = 20
    if state == 'late-power-lock':
        props['TimeoutConflicts'][0]['kcm'] = 'kcm_powerdevilprofilesconfig'
if state=='screensaver-off': props.update(State='screensaver-off', ScreensaverOffReason='RAW ERROR screensaver diagnostic detail')
if state=='running': props.update(State='running', RunningSince=now-420)
if state.startswith('unknown-'):
    code = {'unknown-kwin':'bridge-error', 'unknown-initializing':'initializing', 'unknown-policy':'policyagent-unavailable', 'unknown-future':'future-code'}[state]
    props.update(State='unknown', ExactAttribution=code == 'policyagent-unavailable', UnavailableCode=code,
                 UnavailableReason='RAW ERROR org.freedesktop.DBus.Error.Failed: diagnostic detail')
if state == 'timeout-unknown': props['ScreensaverTimeout'] = 0
if state == 'ready-one': props['ScreensaverTimeout'] = 89
if state=='empty': history=[]
if state in ['empty','blocked-many','blocked-one','unknown-kwin']:
    props['LockSleepBlockers']=[dict(appName='Elisa',iconName='elisa',reason='Playing music',what='sleep',source='powerdevil',mode='block')]
if state=='blocked-many':
    props['LockSleepBlockers'] += [dict(appName='Google Chrome',iconName='google-chrome',reason='WebRTC has active PeerConnections',what=w,source='powerdevil',mode='block') for w in ['idle','sleep']]
if state=='long':
    props['Blockers']=[dict(chrome, caption='Quarterly planning review and roadmap discussion for next year – Google Meet',since=now-86400),dict(firefox,caption=''),window('steam_app_367520','','Hollow Knight',now-1620,'steam-id'),dict(haruna,caption='The.Long.Documentary.Series.S01E04.The.Episode.Title.2160p.HDR.mkv'),window('VLC media player','vlc','Concert recording.mkv',now-420,'vlc-id')]
if state == 'markup':
    firefox.update(appName='<b>Firefox & Co</b>', caption='Report — Firefox <img src="file:///missing">')
    props.update(State='blocked', Blockers=[firefox], LockSleepBlockers=[dict(appName='<b>Elisa</b>', iconName='elisa', reason='<b>Playing & music</b>', what='sleep')])
    history = [interval(firefox, now-3600, now)]
if state == 'caption':
    firefox['caption'] = 'Report — Firefox'
    props.update(State='blocked', Blockers=[firefox])
    history = [interval(firefox, now-3600, now)]
if state == 'retention':
    history = [interval(firefox, now-7*86400-3600, now-7*86400+60)]
signatures={'State':'s','ExactAttribution':'b','UnavailableCode':'s','UnavailableReason':'s','ScreensaverTimeout':'u','Blockers':'aa{sv}','BlockedUnattributed':'b','LockSleepBlockers':'aa{sv}','TimeoutConflicts':'aa{sv}','RunningSince':'x','ScreensaverOffReason':'s'}
if state == 'timeout-missing':
    del signatures['ScreensaverTimeout']
    del props['ScreensaverTimeout']
def rows(items):
    return [{k:GLib.Variant('b' if isinstance(v,bool) else 'u' if k == 'seconds' else 'x' if isinstance(v,int) else 's',v) for k,v in row.items()} for row in items]
def variant(key,value): return GLib.Variant(signatures[key], rows(value) if isinstance(value,list) else value)
xml='<node><interface name="'+IFACE+'">'+''.join(f'<property name="{k}" type="{v}" access="read"/>' for k,v in signatures.items())+'''
<method name="History"><arg type="u" direction="in"/><arg type="aa{sv}" direction="out"/></method>
<method name="ClearHistory"/><method name="ActivateWindow"><arg type="s" direction="in"/><arg type="b" direction="out"/></method>
<method name="StartScreensaver"><arg type="b" direction="out"/></method>
<method name="ClaimReturnNotice"><arg type="u" direction="in"/><arg type="b" direction="out"/></method>
<signal name="Changed"/><signal name="BlockedWhileAway"><arg type="u"/><arg type="x"/><arg type="x"/><arg type="aa{sv}"/></signal>
</interface></node>'''
bus=Gio.bus_get_sync(Gio.BusType.SESSION,None)
replacement = None
hold_requests = False
pending_requests = []
def log(text):
    print(text,flush=True)
def changed(): bus.emit_signal(None,PATH,IFACE,'Changed',None)
def method(conn,sender,path,iface,name,args,invocation):
    global history
    log('CALL '+name+' '+str(args.unpack()))
    if conn == bus and hold_requests:
        pending_requests.append((name, invocation))
        return
    if conn == replacement:
        if name == 'History': invocation.return_value(GLib.Variant('(aa{sv})', ([],)))
        elif name == 'ClearHistory': invocation.return_value(None)
        else: invocation.return_value(GLib.Variant('(b)', (False,)))
        return
    if name=='History' and state == 'history-error':
        invocation.return_dbus_error(IFACE+'.Storage', 'RAW ERROR history storage detail')
    elif name=='ClearHistory' and state == 'clear-error':
        invocation.return_dbus_error(IFACE+'.Storage', 'RAW ERROR clear storage detail')
    elif name=='History': invocation.return_value(GLib.Variant('(aa{sv})',(rows(history),)))
    elif name=='ClaimReturnNotice':
        ident = args.unpack()[0]
        claimed = ident in unclaimed_notices
        if claimed: unclaimed_notices.remove(ident)
        log('CLAIM '+str(claimed)); invocation.return_value(GLib.Variant('(b)',(claimed,)))
    elif name=='ActivateWindow': invocation.return_value(GLib.Variant('(b)',(True,)))
    elif name=='StartScreensaver':
        invocation.return_value(GLib.Variant('(b)',(True,)))
        Gio.bus_own_name_on_connection(bus,'org.kde.PlasmaVisualScreensaver',Gio.BusNameOwnerFlags.NONE,None,None)
        props['State']='ready'; changed()
    else:
        history=[]; unclaimed_notices.clear(); invocation.return_value(None); changed()
info=Gio.DBusNodeInfo.new_for_xml(xml)
def get_property(c,s,p,i,k):
    if state == 'loading' and k == 'State': time.sleep(4)
    if c == replacement and k == 'State': return GLib.Variant('s', 'running')
    return variant(k,props[k])
bus.register_object(PATH,info.interfaces[0],method,get_property,None)
if state!='service-down': Gio.bus_own_name_on_connection(bus,NAME,Gio.BusNameOwnerFlags.ALLOW_REPLACEMENT,None,None)
start_xml='''<node><interface name="org.example.Fixture"><method name="StartService"/><method name="EmitReturn"/>
<method name="HoldRequests"/><method name="PendingCount"><arg type="i" direction="out"/></method>
<method name="ReplaceOwner"/><method name="ReleaseOldReplies"/><method name="RestoreOwner"/>
</interface></node>'''
def request_name(connection):
    # Both owners permit a direct replacement, with no unregistered interval.
    reply = connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
        'org.freedesktop.DBus', 'RequestName', GLib.Variant('(su)', (NAME, 7)),
        GLib.VariantType.new('(u)'), Gio.DBusCallFlags.NONE, 2000, None)
    assert reply.unpack()[0] in (1, 4), reply.unpack()
def start_service(c,s,p,i,n,a,inv):
    global replacement, hold_requests
    if n == 'HoldRequests': hold_requests = True; inv.return_value(None); return
    if n == 'PendingCount': inv.return_value(GLib.Variant('(i)', (len(pending_requests),))); return
    if n == 'ReplaceOwner':
        replacement = Gio.DBusConnection.new_for_address_sync(os.environ['DBUS_SESSION_BUS_ADDRESS'],
            Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
        replacement.register_object(PATH, info.interfaces[0], method, get_property, None)
        request_name(replacement)
        inv.return_value(None); return
    if n == 'ReleaseOldReplies':
        for name, invocation in pending_requests:
            if name == 'History': invocation.return_value(GLib.Variant('(aa{sv})', (rows(history),)))
            elif name == 'ClearHistory': invocation.return_value(None)
            else: invocation.return_value(GLib.Variant('(b)', (name == 'ClaimReturnNotice',)))
        pending_requests.clear(); hold_requests = False
        inv.return_value(None); return
    if n == 'RestoreOwner':
        request_name(bus)
        replacement.close_sync(None); replacement = None
        inv.return_value(None); return
    if n == 'EmitReturn':
        GLib.timeout_add(500,notice); inv.return_value(None); return
    Gio.bus_own_name_on_connection(bus,NAME,Gio.BusNameOwnerFlags.NONE,None,None)
    log('SERVICE STARTED'); inv.return_value(None)
bus.register_object('/Fixture',Gio.DBusNodeInfo.new_for_xml(start_xml).interfaces[0],start_service,None,None)
Gio.bus_own_name_on_connection(bus,'org.example.Fixture',Gio.BusNameOwnerFlags.NONE,None,None)
# Fake notification server: captures KNotification; never activates a desktop server.
notify_xml='''<node><interface name="org.freedesktop.Notifications">
<method name="GetCapabilities"><arg type="as" direction="out"/></method>
<method name="GetServerInformation"><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/></method>
<method name="Notify"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/></method>
<method name="CloseNotification"><arg type="u" direction="in"/></method>
<signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
<signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
</interface></node>'''
def notify_method(c,s,p,i,name,args,inv):
    if name=='GetCapabilities': inv.return_value(GLib.Variant('(as)',(['actions','body','body-markup'],)))
    elif name=='GetServerInformation': inv.return_value(GLib.Variant('(ssss)',('Fake','Tests','1','1.2')))
    elif name=='Notify':
        log('NOTIFICATION '+str(args.unpack())); inv.return_value(GLib.Variant('(u)',(1,)))
        GLib.timeout_add(100, lambda: (bus.emit_signal(None,'/org/freedesktop/Notifications','org.freedesktop.Notifications','ActionInvoked',GLib.Variant('(us)',(1,'default'))),False)[1])
    else: inv.return_value(None)
bus.register_object('/org/freedesktop/Notifications',Gio.DBusNodeInfo.new_for_xml(notify_xml).interfaces[0],notify_method,None,None)
Gio.bus_own_name_on_connection(bus,'org.freedesktop.Notifications',Gio.BusNameOwnerFlags.NONE,None,None)
def notice():
    global notice_id
    notice_id += 1
    unclaimed_notices.add(notice_id)
    windows = [{key:row[key] for key in ['appName', 'iconName', 'caption']} | dict(seconds=seconds)
               for row,seconds in [(haruna,19800), (firefox,600)]]
    if state in ['caption', 'markup']:
        windows = [dict(appName=firefox['appName'], iconName=firefox['iconName'], caption=firefox['caption'], seconds=19800)]
    if state == 'unidentified':
        windows = [dict(appName='Unidentified window', iconName='preferences-system-windows', caption='', seconds=19800)]
    bus.emit_signal(None,PATH,IFACE,'BlockedWhileAway',GLib.Variant('(uxxaa{sv})',(notice_id,now-19800,now,rows(windows)))); log('EMIT BlockedWhileAway'); return False
log('READY '+state)
GLib.MainLoop().run()

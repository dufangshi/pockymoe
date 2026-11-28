import json, sys

def send(v): print(json.dumps(v), flush=True)
def done():
    send({'method':'item/agentMessage/delta','params':{'threadId':'shared','itemId':'message','delta':'Hello'}})
    send({'method':'turn/completed','params':{'threadId':'shared','turn':{'id':'native-turn','status':'completed'}}})
approval='command'
startup_burst=False
for line in sys.stdin:
    v=json.loads(line); m=v.get('method'); p=v.get('params',{}); result={}
    if not m:
        if v.get('id')==900:
            if approval=='permissions':
                assert v['result']['permissions']=={'network':{'enabled':True}} and v['result']['scope']=='turn' and v['result']['strictAutoReview'] is False
            else: assert v['result']['decision']=='accept'
            done()
        continue
    if 'id' not in v: continue
    if m in ('thread/start','thread/resume','thread/fork'):
        startup_burst=p.get('model')=='startup-burst'
        if startup_burst:
            for i in range(32):
                send({'method':'mcpServer/startupStatus/updated','params':{'name':f'docs-{i}','status':'ready'}})
            send({'method':'mcpServer/startupStatus/updated','params':{'name':'broken-docs','status':'failed','error':'fixture startup failed'}})
        result={'thread':{'id':'fork' if m=='thread/fork' else 'shared'},'model':'test'}
        if m!='thread/start':
            assert p.get('excludeTurns') is True, 'native load must request a metadata-only response'
    if m=='thread/settings/update' and startup_burst:
        for i in range(32):
            send({'method':'mcpServer/startupStatus/updated','params':{'name':f'docs-{i}','status':'ready'}})
    if m=='thread/turns/list': result={'data':[{'id':'old-turn'}], 'nextCursor':None}
    if m=='thread/revert': assert p['beforeTurnId']=='old-turn'
    if m=='turn/start': result={'turn':{'id':'native-turn'}}
    if m=='review/start':
        assert p['target']=={'type':'baseBranch','branch':'main'} and p['delivery']=='inline', p
        result={'turn':{'id':'native-turn'},'reviewThreadId':'shared'}
    if m=='mcpServerStatus/list':
        result={'data':[{'name':'docs','runtimeStatus':'connected','authStatus':'unsupported','tools':{'search':{}}}],'nextCursor':None}
    send({'id':v['id'],'result':result})
    if m=='turn/start':
        text=p['input'][0]['text']
        if text=='crash': sys.exit(0)
        if text=='make a plan':
            assert p['collaborationMode']['mode']=='plan', p
            send({'method':'item/completed','params':{'threadId':'shared','item':{'id':'plan-1','type':'plan','text':'1. Add tests'}}})
            send({'method':'turn/completed','params':{'threadId':'shared','turn':{'id':'native-turn','status':'completed'}}})
            continue
        if text=='Implement the approved plan.':
            assert p['collaborationMode']['mode']=='default', p
            send({'method':'item/agentMessage/delta','params':{'threadId':'shared','itemId':'impl','delta':'Implemented'}})
            send({'method':'turn/completed','params':{'threadId':'shared','turn':{'id':'native-turn','status':'completed'}}})
            continue
        if text=='approval':send({'id':900,'method':'item/commandExecution/requestApproval','params':{'threadId':'shared','command':'echo hello','reason':'test'}})
        elif text=='approval-permissions':
            approval='permissions'
            send({'id':900,'method':'item/permissions/requestApproval','params':{'threadId':'shared','reason':'Need network','permissions':{'network':{'enabled':True}}}})
        elif text=='notices':
            send({'method':'warning','params':{'threadId':'shared','message':'Model metadata for x not found.'}})
            send({'method':'remoteControl/status/changed','params':{'status':'disabled'}})
            done()
        elif text=='settled':
            # Codex settles its own callback (e.g. superseded); the card must go.
            import time
            send({'id':901,'method':'item/commandExecution/requestApproval','params':{'threadId':'shared','itemId':'c','command':'ls'}})
            time.sleep(0.3)
            send({'method':'serverRequest/resolved','params':{'threadId':'shared','requestId':901}})
            # Settled before its request is read, and one left open at the end.
            send({'method':'serverRequest/resolved','params':{'threadId':'shared','requestId':902}})
            send({'id':902,'method':'item/commandExecution/requestApproval','params':{'threadId':'shared','itemId':'d','command':'pwd'}})
            send({'id':903,'method':'item/fileChange/requestApproval','params':{'threadId':'shared','itemId':'e'}})
            time.sleep(0.3)
            done()
        elif text=='limits':
            # Sparse rolling update: a per-model alias limit is ignored, then
            # the account's own codex limit reports only its weekly window.
            send({'method':'account/rateLimits/updated','params':{'rateLimits':{'limitId':'codex_other','primary':{'usedPercent':99,'windowDurationMins':300,'resetsAt':4102444800}}}})
            send({'method':'account/rateLimits/updated','params':{'rateLimits':{'limitId':'codex','primary':None,'secondary':{'usedPercent':42,'windowDurationMins':10080,'resetsAt':4102444800}}}})
            done()
        elif text!='wait': done()

    if m=='thread/compact/start':
        send({'method':'turn/started','params':{'threadId':'shared','turn':{'id':'compact-turn'}}})
        send({'method':'item/completed','params':{'threadId':'shared','turnId':'compact-turn','item':{'id':'c','type':'contextCompaction'}}})
        send({'method':'thread/tokenUsage/updated','params':{'threadId':'shared','tokenUsage':{'total':{'totalTokens':10},'last':{'totalTokens':5},'modelContextWindow':100}}})
        send({'method':'turn/completed','params':{'threadId':'shared','turn':{'id':'compact-turn','status':'completed'}}})

    if m=='review/start':
        send({'method':'item/completed','params':{'threadId':'shared','item':{'id':'rv','type':'exitedReviewMode','review':'No issues found.'}}})
        send({'method':'turn/completed','params':{'threadId':'shared','turn':{'id':'native-turn','status':'completed'}}})
    if m=='turn/settings/update':
        assert p['turnId']=='native-turn' and p['model']=='live-model' and 'effort' not in p, p
    if m=='turn/steer': done()
    if m=='turn/interrupt':
        assert p['turnId']=='native-turn'
        send({'method':'turn/completed','params':{'threadId':'shared','turn':{'id':'native-turn','status':'interrupted'}}})

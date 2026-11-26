import json, sys

def send(v): print(json.dumps(v), flush=True)
def done():
    send({'method':'item/agentMessage/delta','params':{'threadId':'shared','itemId':'message','delta':'Hello'}})
    send({'method':'turn/completed','params':{'threadId':'shared','turn':{'id':'native-turn','status':'completed'}}})
approval='command'
for line in sys.stdin:
    v=json.loads(line); m=v.get('method'); p=v.get('params',{}); result={}
    if not m:
        if v.get('id')==900:
            if approval=='permissions':
                assert v['result']=={'permissions':{'network':{'enabled':True}},'scope':'turn'}
            else: assert v['result']['decision']=='accept'
            done()
        continue
    if 'id' not in v: continue
    if m in ('thread/start','thread/resume','thread/fork'):
        result={'thread':{'id':'fork' if m=='thread/fork' else 'shared'},'model':'test'}
    if m=='thread/turns/list': result={'data':[{'id':'old-turn'}], 'nextCursor':None}
    if m=='thread/revert': assert p['beforeTurnId']=='old-turn'
    if m=='turn/start': result={'turn':{'id':'native-turn'}}
    send({'id':v['id'],'result':result})
    if m=='turn/start':
        text=p['input'][0]['text']
        if text=='crash': sys.exit(0)
        if text=='approval':send({'id':900,'method':'item/commandExecution/requestApproval','params':{'threadId':'shared','command':'echo hello','reason':'test'}})
        elif text=='approval-permissions':
            approval='permissions'
            send({'id':900,'method':'item/permissions/requestApproval','params':{'threadId':'shared','reason':'Need network','permissions':{'network':{'enabled':True}}}})
        elif text!='wait': done()

    if m=='turn/steer': done()
    if m=='turn/interrupt':
        assert p['turnId']=='native-turn'
        send({'method':'turn/completed','params':{'threadId':'shared','turn':{'id':'native-turn','status':'interrupted'}}})

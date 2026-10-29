"""Fast Alice webhook; only commands and status cross the cloud."""
import hashlib
import hmac
import json
import os
import re
import time
from datetime import datetime,timedelta,timezone

from flask import Flask,jsonify,request
app=Flask(__name__);app.config['MAX_CONTENT_LENGTH']=16384
_db=_publisher=None


def db():
    global _db
    if _db is None:
        from google.cloud import firestore
        _db=firestore.Client(database=os.environ['FIRESTORE_DATABASE'])
    return _db


def publisher():
    global _publisher
    if _publisher is None:
        from google.cloud import pubsub_v1
        _publisher=pubsub_v1.PublisherClient(publisher_options=pubsub_v1.types.PublisherOptions(enable_message_ordering=True))
    return _publisher


def surface(body):
    appid=body.get('session',{}).get('application',{}).get('application_id')
    return hashlib.sha256((body['session']['skill_id']+':'+appid).encode()).hexdigest() if appid else None


def owner(body):
    who=body['session'].get('user',{}).get('user_id') or ''
    return hashlib.sha256(who.encode()).hexdigest()


def answer(text,end=False):
    return jsonify(version='1.0',response={'text':text[:1024],'end_session':end})


def parse(text):
    text=re.sub(r'\s+',' ',text).strip().strip('.!? ')
    command=text.lower()
    for action,variants in {
        'next':{'дальше','следующая','следующий трек','следующую','следующую песню','переключи песню','включи следующую песню'},
        'previous':{'назад','предыдущая','предыдущую','предыдущий трек'},
        'pause':{'стоп','пауза','останови','поставь на паузу','выключи музыку'},
        'resume':{'продолжай','продолжи','продолжи музыку','продолжить','сними с паузы'},
        'similar':{'похожие','включи похожие','поставь похожие песни','включи похожую музыку'},
        'now':{'что играет','какая песня','что сейчас играет','название песни',
               'что за песня','что за песня сейчас играет','какая песня сейчас играет',
               'что за трек','что за трек сейчас играет','кто поёт','кто поет'},
    }.items():
        if command in variants:return action,''
    query=re.sub(r'^(?:включи(?:ть)?|поставь|поставить|сыграй|найди|хочу послушать)\s+(?:(?:песню|трек|музыку)\s+)?','',text,flags=re.I)
    return 'play',query[:500]


@app.get('/healthz')
def health():return jsonify(ok=True)


@app.post('/alice/<token>')
def alice(token):
    if not hmac.compare_digest(token,os.environ.get('WEBHOOK_TOKEN','') or '-'):return '',404
    body=request.get_json(silent=True)
    if not isinstance(body,dict):return '',400
    session=body.get('session');req=body.get('request')
    if not isinstance(session,dict) or not isinstance(req,dict):return '',400
    if session.get('skill_id')!=os.environ.get('ALICE_SKILL_ID'):return '',403
    if req.get('type')!='SimpleUtterance':return answer('Назовите песню голосом.',False)
    raw=req.get('command',req.get('original_utterance',''))
    if not isinstance(raw,str):return '',400
    raw=re.sub(r'^(?:скажи|передай|попроси)\s+(?:навыку\s+)?(?:мою фонотеку|моя фонотека)\s*','',raw,flags=re.I).strip()
    command=raw.lower().strip('.!? ')
    if not command or command in {'привет','готов','готовность'}:return answer('Что включить?')
    if command in {'помощь','что ты умеешь'}:return answer('Назовите песню. Команды: дальше, пауза, продолжай, что играет.',False)
    if command in {'выход','выйди из навыка','закрой навык','хватит'}:return answer('Готово.',True)
    appid=surface(body)
    if not appid:return answer('Для музыки нужна привязанная Станция.')
    try:
        binding=re.fullmatch(r'привязка ([\d ]+)',command)
        if binding:
            if not hmac.compare_digest(binding[1].replace(' ',''),os.environ.get('BIND_CODE','-')):
                return answer('Код подключения не подходит.')
            db().collection('music_surfaces').document(appid).set({'owner':owner(body),'station':'home'},timeout=.7,retry=None)
            return answer('Подключено.')
        route=db().collection('music_surfaces').document(appid).get(timeout=.7,retry=None)
        if not route.exists or route.to_dict().get('owner')!=owner(body):return answer('Эта колонка ещё не подключена к музыкальному сервису.')
        result=re.fullmatch(r'результат ([\d ]+)',command)
        if result:
            from google.cloud.firestore_v1.base_query import FieldFilter
            docs=db().collection('music_jobs').where(filter=FieldFilter('code','==',result[1].replace(' ',''))).limit(1).get(timeout=.7,retry=None)
            if not docs:return answer('Этот ответ устарел.')
            data=docs[0].to_dict()
            if data.get('surface')!=appid or data.get('owner')!=owner(body):return answer('Этот ответ недоступен.')
            return answer(data.get('answer','Команда ещё выполняется.'))
        agent=db().collection('music_state').document('home').get(timeout=.7,retry=None)
        info=agent.to_dict() if agent.exists else {}
        if time.time()-info.get('last_seen',0)>75:return answer('Музыкальный сервер сейчас недоступен. Попробуйте ещё раз чуть позже.')
        if not info.get('status',{}).get('station_connected'):
            return answer('Не вижу привязанную Станцию. Проверьте её подключение к домашней сети.')
        action,query=parse(raw)
        current_answer=None
        if action=='now':
            current=info.get('status',{}).get('current') or {}
            current_answer=(current.get('title','')+', '+current.get('artist','')).strip(', ')
            if not current_answer:return answer('Музыка ещё не включена.')
        sid=session.get('session_id');mid=session.get('message_id')
        if not isinstance(sid,str) or not isinstance(mid,int):return '',400
        job=hashlib.sha256((appid+':'+sid+':'+str(mid)).encode()).hexdigest()
        code=str(10**17+int(job[:14],16)%(9*10**17))
        ref=db().collection('music_jobs').document(job)
        from google.api_core.exceptions import AlreadyExists
        try:ref.create({'surface':appid,'owner':owner(body),'code':code,'status':'pending',
                        'expires_at':datetime.now(timezone.utc)+timedelta(hours=1)},timeout=.7,retry=None)
        except AlreadyExists:pass  # Native receiver deduplicates the stable job ID before execution.
        command_data={'id':job,'code':code,'action':action,'query':query,'expires_at':time.time()+45}
        topic=publisher().topic_path(os.environ['GOOGLE_CLOUD_PROJECT'],'alice-music-home')
        publisher().publish(topic,json.dumps(command_data,ensure_ascii=False).encode(),ordering_key='home').result(timeout=1.3)
        texts={'next':'Дальше.','previous':'Предыдущая.',
               'pause':'Пауза.','resume':'Продолжаю.','similar':'Похожие.'}
        return answer(current_answer or texts.get(action,'Включаю.'))
    except Exception as exc:
        app.logger.warning('Music webhook: %s',type(exc).__name__)
        return answer('Не получилось передать команду. Повторите через несколько секунд.')


@app.post('/agent')
def agent():
    expected='Bearer '+os.environ.get('AGENT_TOKEN','-')
    if not hmac.compare_digest(request.headers.get('Authorization',''),expected):return '',403
    body=request.get_json(silent=True) or {}
    status=body.get('status',{})
    db().collection('music_state').document('home').set({'last_seen':time.time(),'status':status},timeout=3)
    result=body.get('result',{})
    if result and re.fullmatch(r'[a-f0-9]{64}',result.get('id','')):
        ref=db().collection('music_jobs').document(result['id'])
        if ref.get(timeout=3).exists:ref.update({'status':'done' if result.get('ok') else 'error','answer':str(result.get('answer',''))[:1000]},timeout=3)
    return '',204

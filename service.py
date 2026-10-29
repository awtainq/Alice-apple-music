"""Native local service. Receives authenticated cloud commands via outbound Pub/Sub."""
import asyncio
import argparse
import json
import os
import secrets
import sqlite3
import time
from pathlib import Path

import aiohttp
from aiohttp import web
from engine import Engine
from voice import vocalize,Voice


async def run(args):
    private=Path(args.private).resolve();cache=Path(args.cache).resolve();cache.mkdir(parents=True,exist_ok=True)
    conf=json.loads((private/'config.json').read_text())
    db=sqlite3.connect(private/'commands.sqlite3');db.execute('CREATE TABLE IF NOT EXISTS done (id TEXT PRIMARY KEY, timestamp REAL)');db.commit()
    async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=90)) as session:
        engine=Engine(conf,private,cache,session);await engine.apple.initialize()
        async def control(request):
            if not secrets.compare_digest(request.headers.get('Authorization',''),'Bearer '+conf['agent_token']):
                raise web.HTTPUnauthorized()
            if request.method=='GET':return web.json_response(engine.status())
            body=await request.json()
            try:
                if body['action']=='announce_now':
                    message=await engine.command('now',voice=True)
                    confirmed=await vocalize(message,port=conf.get('recognition_port',19875))
                    print(json.dumps({'event':'local_title_tts','confirmed':confirmed}),flush=True)
                else:message=await engine.command(body['action'],body.get('query',''),voice=body.get('voice') is True)
                return web.json_response({'message':message,'status':engine.status()})
            except Exception as exc:
                message=str(exc) if isinstance(exc,RuntimeError) else 'Не удалось выполнить команду.'
                print(json.dumps({'event':'api_command_failed','action':body.get('action'),'error_type':type(exc).__name__}),flush=True)
                return web.json_response({'error':message,'error_type':type(exc).__name__},status=502)
        async def health(request):return web.json_response({'ok':True,'native':True})
        app=web.Application(client_max_size=8192)
        app.router.add_get('/healthz',health)
        app.router.add_get('/audio/{token}/{song}.m4a',engine.serve_audio)
        app.router.add_get('/api',control);app.router.add_post('/api',control)
        runner=web.AppRunner(app);await runner.setup()
        await web.TCPSite(runner,conf['bind_host'],conf['port']).start()
        print(json.dumps({'event':'ready','port':conf['port']}),flush=True)
        tasks=[asyncio.create_task(engine.station.run()),asyncio.create_task(engine.watch())]
        if conf.get('airplay_bus'):
            from airplay import AirplayMonitor
            tasks.append(asyncio.create_task(AirplayMonitor(engine,conf['airplay_bus']).run()))
        if args.local_voice:
            voice=Voice(conf,session)
            tasks.extend([asyncio.create_task(voice.read('127.0.0.1',conf.get('recognition_port',9875))),
                          asyncio.create_task(voice.execute()),
                          asyncio.create_task(voice.local_spotter(conf.get('spotter_log','/data/quasar/daemons_logs/maind.log')))])
        subscriber=None;pull_future=None
        async def report(result=None):
            if not conf.get('relay_url'):return
            payload={'status':engine.status()}
            if result:payload['result']=result
            try:
                async with session.post(conf['relay_url']+'/agent',headers={'Authorization':'Bearer '+conf['agent_token']},
                                        json=payload,timeout=aiohttp.ClientTimeout(total=12)) as r:
                    r.raise_for_status()
            except Exception as exc:print('Cloud report:',type(exc).__name__,flush=True)
        async def heartbeat():
            try:await asyncio.wait_for(engine.station.ready.wait(),15)
            except TimeoutError:pass
            while True:
                await report();await asyncio.sleep(15 if engine.state=='playing' else 30)
        async def activate_skill():
            try:
                await asyncio.wait_for(engine.station.ready.wait(),120)
                await report()
                await engine.station.send({'command':'sendText','text':'скажи навыку '+conf['activation_name']+' привет'})
                print('Voice skill opened on startup.',flush=True)
            except Exception as exc:print('Startup skill:',type(exc).__name__,flush=True)
        async def handle(message):
            data=json.loads(message.data);job=data.get('id','')
            if not isinstance(job,str) or len(job)!=64 or data.get('expires_at',0)<time.time():message.ack();return
            if db.execute('SELECT 1 FROM done WHERE id=?',(job,)).fetchone():message.ack();return
            db.execute('INSERT INTO done VALUES (?,?)',(job,time.time()));db.commit()
            started=time.monotonic()
            print(json.dumps({'event':'voice_command_received','job':job[:10],'action':data['action']}),flush=True)
            try:answer=await engine.command(data['action'],data.get('query',''),voice=True);ok=True
            except Exception as exc:
                answer=str(exc) if isinstance(exc,RuntimeError) else 'Не получилось выполнить музыкальную команду.'
                ok=False;print('Voice command:',type(exc).__name__,flush=True)
            print(json.dumps({'event':'voice_command_finished','job':job[:10],'ok':ok,'seconds':round(time.monotonic()-started,2)}),flush=True)
            await report({'id':job,'ok':ok,'answer':answer})
            message.ack()
            # Only failures need a separate spoken reply. Successful audio starts directly.
            if not ok and conf.get('activation_name'):
                try:await engine.station.send({'command':'sendText','text':'скажи навыку '+conf['activation_name']+' результат '+data['code']})
                except Exception as exc:print('Failure notification:',type(exc).__name__,flush=True)
        if not args.no_cloud:
            from google.cloud import pubsub_v1
            from google.oauth2 import service_account
            credentials=service_account.Credentials.from_service_account_file(private/'pubsub-home.json')
            subscriber=pubsub_v1.SubscriberClient(credentials=credentials)
            loop=asyncio.get_running_loop()
            def callback(message):
                try:asyncio.run_coroutine_threadsafe(handle(message),loop).result(timeout=100)
                except Exception:message.nack()
            pull_future=subscriber.subscribe(subscriber.subscription_path(conf['project'],conf['subscription']),
                callback=callback,flow_control=pubsub_v1.types.FlowControl(max_messages=1))
            tasks.append(asyncio.create_task(heartbeat()))
            if conf.get('open_skill_on_start') and conf.get('activation_name'):
                tasks.append(asyncio.create_task(activate_skill()))
        try:await asyncio.Event().wait()
        finally:
            if pull_future:pull_future.cancel()
            if subscriber:subscriber.close()
            for task in tasks:task.cancel()
            await asyncio.gather(*tasks,return_exceptions=True)
            await runner.cleanup();db.close()


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--private',required=True);parser.add_argument('--cache',required=True)
    parser.add_argument('--no-cloud',action='store_true')
    parser.add_argument('--local-voice',action='store_true',help='Read recognition and bare commands locally on a rooted Station')
    args=parser.parse_args()
    try:asyncio.run(run(args))
    except KeyboardInterrupt:pass

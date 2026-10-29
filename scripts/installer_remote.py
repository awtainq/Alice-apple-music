"""Transactional Station-side installer; shipped inside the public ARM package."""
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from urllib.request import Request,urlopen


def switch_install(stage,target,backup,start,stop,probe):
    """Keep the current version usable until staging is ready; rollback on failure."""
    obsolete=backup.with_name(backup.name+'.obsolete')
    if obsolete.exists():raise RuntimeError('Остался незавершённый откат: '+str(obsolete))
    old_exists=target.exists()
    if old_exists:stop(target)
    moved=False;installed=False
    try:
        if backup.exists():backup.rename(obsolete)
        if old_exists:target.rename(backup);moved=True
        stage.rename(target);installed=True
        start(target);probe(target)
    except Exception:
        if installed and target.exists():
            stop(target)
            failed=stage.with_name(stage.name+'.failed')
            target.rename(failed)
        if moved:backup.rename(target);start(target)
        elif old_exists and target.exists():start(target)
        if obsolete.exists():obsolete.rename(backup)
        raise
    else:
        if obsolete.exists():shutil.rmtree(obsolete)


def launch(target):
    (target/'launch.sh').chmod(0o700)
    subprocess.run([str(target/'launch.sh'),'start'],check=True)


def stop(target):
    launcher=target/'launch.sh'
    if not launcher.exists():return
    try:
        config=json.loads((target/'private/config.json').read_text())
        request=Request('http://127.0.0.1:8099/api',data=b'{"action":"pause"}',
                        headers={'Authorization':'Bearer '+config['agent_token'],'Content-Type':'application/json'})
        with urlopen(request,timeout=5):pass
    except (OSError,ValueError,KeyError):pass
    subprocess.run([str(launcher),'stop'],check=True)
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        pidfile=Path('/tmp/alice-music.pid')
        if not pidfile.exists():return
        try:
            pid=int(pidfile.read_text());os.kill(pid,0)
        except (ValueError,ProcessLookupError,FileNotFoundError):return
        time.sleep(.2)
    raise RuntimeError('Прежний процесс не завершился; новая версия не запускалась.')


def probe(target):
    config=json.loads((target/'private/config.json').read_text())
    deadline=time.monotonic()+60
    while time.monotonic()<deadline:
        try:
            request=Request('http://127.0.0.1:8099/api',headers={'Authorization':'Bearer '+config['agent_token']})
            with urlopen(request,timeout=3) as response:status=json.load(response)
            if status.get('station_connected'):
                print('Плеер отвечает, соединение со Станцией установлено.',flush=True);return
        except (OSError,ValueError):pass
        time.sleep(1)
    raise RuntimeError('Проверка запуска не прошла за 60 секунд. Выполняется откат.')


def install_boot(target):
    boot=Path('/etc/init.d/S99zzalice-music')
    if boot.exists() and '/data/opt/alice-music' not in boot.read_text():
        raise RuntimeError('Найден чужой файл автозапуска: автоматическая замена запрещена.')
    subprocess.run(['mount','-o','remount,rw','/'],check=True)
    try:
        shutil.copyfile(target/'S99zzalice-music',boot);boot.chmod(0o755)
        os.sync()
    finally:subprocess.run(['mount','-o','remount,ro','/'],check=True)


def main():
    stage=Path(sys.argv[1]).resolve();target=Path('/data/opt/alice-music');backup=target.with_name('alice-music.previous')
    if stage.parent!=target.parent or not stage.name.startswith('.alice-music-stage-'):
        raise RuntimeError('Недопустимый staging-каталог.')
    # Check boot ownership before stopping the existing application.
    boot=Path('/etc/init.d/S99zzalice-music')
    if boot.exists() and '/data/opt/alice-music' not in boot.read_text():raise RuntimeError('Чужой автозапуск не изменён.')
    private=stage/'private'
    explicit_queue=(private/'queue.json').exists()
    if not private.exists():
        if not (target/'private').is_dir():raise RuntimeError('Нет личного профиля.')
        shutil.copytree(target/'private',private)
    elif (target/'private/queue.json').exists() and not (private/'queue.json').exists():
        shutil.copyfile(target/'private/queue.json',private/'queue.json')
    private.chmod(0o700)
    for name in ('config.json','client_id.bin','private_key.pem'):
        if not (private/name).is_file():raise RuntimeError('Неполный профиль: '+name)
        (private/name).chmod(0o600)
    if not (stage/'vendor').is_dir():raise RuntimeError('Отсутствуют ARM-библиотеки.')
    # Validate native imports without using account credentials or starting a second player.
    env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1')
    subprocess.run(['/opt/bin/python3',str(stage/'check_runtime.py')],check=True,env=env)
    def ready(current):
        probe(current);install_boot(current)
    def stop_and_save(current):
        stop(current)
        if not explicit_queue and stage.exists() and (current/'private/queue.json').exists():
            shutil.copyfile(current/'private/queue.json',stage/'private/queue.json')
    switch_install(stage,target,backup,launch,stop_and_save,ready)
    print('Обновление завершено. Автозапуск установлен; личный профиль сохранён.',flush=True)


if __name__=='__main__':
    try:main()
    except Exception as error:
        print('Ошибка установки: '+str(error),file=sys.stderr);sys.exit(1)

import json
import time
from pathlib import Path
from subprocess import Popen
from time import sleep

import psutil


# TODO: make Windows-friendly
dir = Path("~/.local/share/JetBrains/Toolbox/").expanduser()
bin = dir / "bin/jetbrains-toolbox"
# conf = dir / ".settings.json"
confs = dir / "channels"
log = dir / "logs/toolbox.log"


def add_update_param(d):
    if "autoUpdate" in d["channel"]:
        if d["channel"]["autoUpdate"] == True:
            # TODO: skip this one then?
            #  Make sure to not reset it with remove_update_param then
            raise Exception("This tool is already updated automatically!")
        else:
            raise Exception("'channel.autoUpdate' is not true! This should not be possible")

    d["channel"]["autoUpdate"] = True


def remove_update_param(d):
    d["channel"].pop("autoUpdate")


def update_conf(file: Path, operation):
    with file.open("r+") as file:
        d = json.load(file)
        operation(d)
        file.seek(0)
        json.dump(d, file)
        file.truncate()


def update_all_confs(operation):
    for file in confs.iterdir():
        update_conf(file, operation)


def main():
    update_all_confs(add_update_param)
    toolbox_was_open = None

    try:
        # Kill toolbox if open
        processes = list(filter(lambda p: p.name() == "jetbrains-toolbox", psutil.process_iter()))
        if len(processes) == 0:
            toolbox_was_open = False
        elif len(processes) == 1:
            process, = processes
            process.terminate()
            process.wait()
            toolbox_was_open = True
        else:
            raise Exception("Multiple toolboxes open? This should not be possible")

        p = Popen([bin, "--minimize"])

        updates = 0
        correct_checksums_expected = 0
        start_time = time.time()
        with log.open("r") as file:
            file.seek(0,2)  # Go to the end of file
            while True:
                # TODO: Unfortunately there is no log message indicating there are no updates; so waiting is necessary it looks like.
                #  Unless we can do something with "Downloaded fus-assistant.xml"? Maybe shorten the time to 1/2 seconds after that message, seems to be fine.
                if updates == 0 and start_time + 10 < time.time():
                    # 10 seconds without recognizing updating, so presuming no update.
                    print("No updates found.")
                    break
                curr_position = file.tell()
                line = file.readline()
                if not line:
                    file.seek(curr_position)
                    sleep(0.1)
                else:
                    # TODO: downloading two updates (one large download, one no download) is currently broken; but that never happens in practice anyway.
                    # If the download is already there, it won't say "Downloading from", but immediately "Correct checksum for".
                    if "Correct checksum for" in line or "Downloading from" in line:
                        if "Correct checksum for" in line and correct_checksums_expected > 0:
                            correct_checksums_expected -= 1
                            continue
                        # Update started
                        print("Found an update, waiting until it finishes...")
                        updates += 1
                        if "Downloading from" in line:
                            # We expect "Correct checksum for" to be broadcast exactly once after the downloading from.
                            correct_checksums_expected += 1
                    elif "Show notification" in line:
                        # Update finished
                        updates -= 1
                        if updates == 0:
                            print("Update finished, exiting...")
                            sleep(2)  # Letting it finish up
                            break
                        else:
                            print("Update finished, waiting for other update(s) to finish")

        p.terminate()
        p.wait()

    finally:  # Still restore original state after crash
        update_all_confs(remove_update_param)
        if toolbox_was_open:
            print("Re-opening...")
            Popen([bin, "--minimize"])
            # Simply exiting now will not quit the toolbox process


if __name__ == "__main__":
    main()

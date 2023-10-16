import subprocess
import shutil
import glob
import os

# 执行 cargo build --release
subprocess.run(['cargo', 'build', '--release'])

# 复制目标文件
source_files = glob.glob('target/release/shell*') + glob.glob('target/release/simdisk*')
destination_folder = 'bin/'

for file_path in source_files:
    if not file_path.endswith('.d'):
        file_name = os.path.basename(file_path)
        destination_path = os.path.join(destination_folder, file_name)
        shutil.copy(file_path, destination_path)
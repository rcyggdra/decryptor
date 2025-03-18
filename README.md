# Batch CIA CCI Decryptor
Batch decrypting for Nintendo 3DS games and applications (.3ds|.cci, .cia).

Azahar makes use of the `.cci` extension, which is the true name of the format used by `.3ds` files.

Batch CIA CCI Decryptor is a rewritten version of the Batch CIA 3DS Decryptor by matiffeder.

Original thread: https://gbatemp.net/threads/batch-cia-3ds-decryptor-a-simple-batch-file-to-decrypt-cia-3ds.512385/

## Features
* DLC/Patch CIA > Decrypted CIA, able to install in Azahar
* 3DS Games > Decrypted and trimmed 3DS, so it is smaller
* CIA Games > Decrypted CCI (NCSD), not CXI (NCCH)
* Auto dectect CIA type (DLC/Patch/Game)

## Usage
* Copy CIA or 3DS files into the root directory containing the batch
* Run "Batch CIA CCI Decryptor.bat"
* Run "Batch CIA CCI Decryptor [Debug].bat" will show command and output logfile，without checking whether the files are encrypted

## Supported operating systems
* Windows 10 x64 or higher

## Credits
* `Batch CIA 3DS Decryptor` - [matiffeder](https://github.com/matiffeder/3DS-stuff)
* `CTRTool.exe/MakeROM.exe` - [3DSGuy](https://github.com/3DSGuy/Project_CTR)
* `seeddb.bin` - [ihavamac](https://github.com/ihaveamac/3DS-rom-tools/tree/master/seeddb)
* `ctrdecrypt` - [shijimasoft](https://github.com/shijimasoft/ctrdecrypt)
* `dlchelper` - [R-YaTian](https://github.com/R-YaTian/3DS-Converters/tree/master/dlchelper)

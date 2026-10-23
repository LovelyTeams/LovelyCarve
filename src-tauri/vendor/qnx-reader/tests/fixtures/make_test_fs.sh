if [ -z "$1" ]
  then
    echo "Supply destination directory!"
    exit 1
else
  if [ ! -d "$1" ]
    then
      echo "Directory not found!"
      exit 1
  fi
fi


dd if=/dev/zero bs=4k count=100 of="$1"/test_image.bin
mkqnx6fs "$1"/test_image.bin
mkdir "$1"/mnt
mount -t qnx6 -o sync=optional "$1"/test_image.bin "$1"/mnt

cd "$1"/mnt
dd bs=1 count=16 < /dev/urandom > this_file_is_small
dd bs=4k count=16 < /dev/urandom > this_file_is_large
dd bs=1k if=/dev/urandom of=this_file_is_large seek=5 count=10 conv=notrunc
dd bs=1 count=16 < /dev/urandom > this_file_is_removed
cp this_file_is_removed this_file_is_a_copy
rm this_file_is_removed
dd bs=1 count=100 < /dev/urandom > this_file_is_not_renamed
mv this_file_is_not_renamed this_file_is_renamed
mkfifo this_is_a_fifo
touch this_file_has_27_characters

mkdir directory_1
mkdir directory_2
mkdir directory_1/directory_1_1
cd directory_1/directory_1_1
echo "This is a test file system!" > message.txt
cd ..
ln -s directory_1_1/message.txt link_message.txt
dd bs=1 count=100 < /dev/urandom > this_file_is_moved
mv this_file_is_moved ../this_file_is_moved
cd ..
ln -s directory_1/link_message.txt link_link_message.txt
ln -s ../directory_2 link_directory_2

cd directory_2
touch read_only
chmod 444 read_only
touch full_permission
chmod 777 full_permission
touch standard_file
touch root_only
chmod 700 root_only
touch executable
chmod 111 executable
touch descending
chmod 764 descending
touch the_weird_one
chmod 002 the_weird_one
touch another_owner
chown nobody another_owner

cd ..
mkdir copy_of_directory_2
cp -r directory_2 copy_of_directory_2

cd "$1"/mnt
tar -czvf "$1"/test_image.tar.gz *
umount "$1"/mnt
rm -r "$1"/mnt
